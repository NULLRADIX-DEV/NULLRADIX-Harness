#![allow(
    clippy::unwrap_used,
    clippy::assert_is_empty,
    reason = "integration tests fail loudly"
)]

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_app::accounts::{AccountFolders, AccountPatch, AccountService};
use nullradix_app::auth::CommandRefusal;
use nullradix_app::secrets::{MemorySecretStore, SecretStore};
use nullradix_app::{
    AppError, ChangeFeed, ChangeOp, ChangeSignal, Entity, ErrorKind, SystemClock, UuidV7Generator,
};
use nullradix_domain::{Account, AccountSlug, ConfigDir, Id, Limit, Version};
use nullradix_store::Store;
use std::sync::Mutex;

/// Folders below `/accounts`; only the listed ones exist.
#[derive(Default)]
struct Folders {
    existing: Mutex<Vec<String>>,
    removed: Mutex<Vec<String>>,
    fail_remove: std::sync::atomic::AtomicBool,
    fail_once: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl AccountFolders for Folders {
    fn root(&self) -> String {
        String::from("/accounts")
    }

    async fn create(&self, slug: &AccountSlug) -> Result<ConfigDir, AppError> {
        let path = format!("/accounts/{slug}");
        self.existing.lock().unwrap().push(path.clone());
        Ok(ConfigDir::from_stored(path))
    }

    async fn check(&self, dir: &ConfigDir) -> Result<(), AppError> {
        if !dir.as_str().starts_with('/') {
            return Err(AppError::invalid("account-invalid", "not absolute"));
        }
        if self
            .existing
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.to_lowercase() == dir.as_str().to_lowercase())
        {
            Ok(())
        } else {
            Err(AppError::invalid("account-invalid", "missing folder"))
        }
    }

    fn key(&self, dir: &ConfigDir) -> String {
        dir.as_str().to_lowercase()
    }

    fn slug_key(&self, slug: &AccountSlug) -> String {
        format!("/accounts/{slug}").to_lowercase()
    }

    fn owns(&self, dir: &ConfigDir) -> bool {
        dir.as_str().starts_with("/accounts/")
    }

    async fn remove(&self, dir: &ConfigDir) -> Result<(), AppError> {
        if self
            .fail_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(AppError::internal(std::io::Error::other("a new file")));
        }
        if self.fail_remove.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AppError::internal(std::io::Error::other(
                "files are in use",
            )));
        }
        self.removed.lock().unwrap().push(dir.as_str().to_owned());
        Ok(())
    }
}

struct Fixture {
    accounts: AccountService,
    store: Arc<Store>,
    secrets: Arc<MemorySecretStore>,
    folders: Arc<Folders>,
}

impl Fixture {
    async fn ops(&self) -> Vec<(Entity, ChangeOp)> {
        self.store
            .changes_since(0, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|change| (change.entity, change.op))
            .collect()
    }

    async fn add(&self, name: &str) -> Account {
        self.accounts.create(name, None, None).await.unwrap()
    }
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let secrets = Arc::new(MemorySecretStore::new());
    let folders = Arc::new(Folders::default());
    let accounts = AccountService::new(
        store.clone(),
        secrets.clone(),
        folders.clone(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
    );
    Fixture {
        accounts,
        store,
        secrets,
        folders,
    }
}

fn code(error: &AppError) -> (ErrorKind, &'static str) {
    (error.kind(), error.code())
}

const TOKEN: &str = "sk-ant-oat01-fake-token";

#[tokio::test]
async fn the_default_account_exists_from_the_start() {
    let f = fixture().await;
    let accounts = f.accounts.list().await.unwrap();
    assert_eq!(accounts.len(), 1);
    let default = &accounts[0];
    assert_eq!(default.slug().as_str(), "default");
    assert_eq!(default.name().as_str(), "Default");
    assert!(default.config_dir().is_none());
    assert!(default.enabled());
    assert!(!default.fallback());
    assert_eq!(default.version(), Version::INITIAL);
    assert_eq!(f.accounts.find("default").await.unwrap().id(), default.id());
}

#[tokio::test]
async fn new_accounts_get_their_own_folder_slug_and_position() {
    let f = fixture().await;
    let max = f.add("Max (private)").await;
    let again = f.add("Max (private)").await;
    assert_eq!(max.slug().as_str(), "max-private");
    assert_eq!(again.slug().as_str(), "max-private-2");
    assert_eq!(max.config_dir().unwrap().as_str(), "/accounts/max-private");
    assert_eq!(max.position(), 1);
    assert_eq!(again.position(), 2);
    let slugs: Vec<String> = f
        .accounts
        .list()
        .await
        .unwrap()
        .iter()
        .map(|account| account.slug().to_string())
        .collect();
    assert_eq!(slugs, ["default", "max-private", "max-private-2"]);
    assert_eq!(f.accounts.get(max.id()).await.unwrap(), max);
}

#[tokio::test]
async fn an_existing_folder_can_be_taken_over_once() {
    let f = fixture().await;
    f.folders
        .existing
        .lock()
        .unwrap()
        .push(String::from("/profiles/work"));
    let work = f
        .accounts
        .create("Work", Some(" /profiles/work "), None)
        .await
        .unwrap();
    assert_eq!(work.config_dir().unwrap().as_str(), "/profiles/work");

    let taken = f
        .accounts
        .create("Other", Some("/PROFILES/work"), None)
        .await
        .unwrap_err();
    assert_eq!(code(&taken), (ErrorKind::Conflict, "account-dir-taken"));
    let relative = f.accounts.create("Rel", Some("profiles/work"), None).await;
    assert_eq!(
        code(&relative.unwrap_err()),
        (ErrorKind::Invalid, "account-invalid")
    );
    let missing = f
        .accounts
        .create("Gone", Some("/profiles/gone"), None)
        .await;
    assert_eq!(
        code(&missing.unwrap_err()),
        (ErrorKind::Invalid, "account-invalid")
    );
    assert_eq!(f.accounts.list().await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_folder_outside_the_accounts_folder_is_refused_when_commands_are_closed() {
    let f = fixture().await;
    f.folders
        .existing
        .lock()
        .unwrap()
        .push(String::from("/profiles/work"));
    let before = f.accounts.list().await.unwrap().len();
    let unconfirmed = f
        .accounts
        .create(
            "Work",
            Some("/profiles/work"),
            Some(CommandRefusal::NeedsStepUp),
        )
        .await
        .unwrap_err();
    assert_eq!(
        code(&unconfirmed),
        (ErrorKind::Forbidden, "step-up-required")
    );
    let token = f
        .accounts
        .create(
            "Work",
            Some("/profiles/work"),
            Some(CommandRefusal::NeedsPerson),
        )
        .await
        .unwrap_err();
    assert_eq!(
        code(&token),
        (ErrorKind::Forbidden, "command-change-needs-person")
    );
    assert_eq!(f.accounts.list().await.unwrap().len(), before);

    let own = f
        .accounts
        .create("Own", None, Some(CommandRefusal::NeedsStepUp))
        .await
        .unwrap();
    assert!(own.config_dir().unwrap().as_str().starts_with("/accounts/"));
    f.folders
        .existing
        .lock()
        .unwrap()
        .push(String::from("/accounts/bob"));
    f.accounts
        .create(
            "Bob",
            Some("/accounts/bob"),
            Some(CommandRefusal::NeedsStepUp),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn names_are_validated() {
    let f = fixture().await;
    let blank = f.accounts.create("  ", None, None).await.unwrap_err();
    assert_eq!(code(&blank), (ErrorKind::Invalid, "account-invalid"));
    let long = f
        .accounts
        .create(&"x".repeat(61), None, None)
        .await
        .unwrap_err();
    assert_eq!(code(&long), (ErrorKind::Invalid, "value-too-long"));
    assert!(f.folders.existing.lock().unwrap().is_empty());
}

#[tokio::test]
async fn updates_change_limits_flags_and_name() {
    let f = fixture().await;
    let max = f.add("Max").await;
    let patch = AccountPatch {
        name: Some(String::from(" Max Pro ")),
        enabled: Some(false),
        fallback: Some(true),
        limit_five_hour: Some(Some(50)),
        limit_week: Some(Some(80)),
    };
    let updated = f
        .accounts
        .update(max.id(), patch, max.version())
        .await
        .unwrap();
    assert_eq!(updated.name().as_str(), "Max Pro");
    assert!(!updated.enabled());
    assert!(updated.fallback());
    assert_eq!(updated.limit_five_hour().map(Limit::percent), Some(50));
    assert_eq!(updated.limit_week().map(Limit::percent), Some(80));
    assert_eq!(updated.version(), max.version().next());
    assert_eq!(f.accounts.get(max.id()).await.unwrap(), updated);

    let cleared = f
        .accounts
        .update(
            max.id(),
            AccountPatch {
                limit_week: Some(None),
                ..AccountPatch::default()
            },
            updated.version(),
        )
        .await
        .unwrap();
    assert_eq!(cleared.limit_week(), None);
    assert_eq!(cleared.limit_five_hour().map(Limit::percent), Some(50));
}

#[tokio::test]
async fn stale_and_invalid_updates_change_nothing() {
    let f = fixture().await;
    let max = f.add("Max").await;
    let stale = f
        .accounts
        .update(
            max.id(),
            AccountPatch {
                enabled: Some(false),
                ..AccountPatch::default()
            },
            Version::from_stored(7),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&stale), (ErrorKind::Conflict, "version-conflict"));
    for bad in [0, 101, -1] {
        let invalid = f
            .accounts
            .update(
                max.id(),
                AccountPatch {
                    limit_five_hour: Some(Some(bad)),
                    ..AccountPatch::default()
                },
                max.version(),
            )
            .await
            .unwrap_err();
        assert_eq!(code(&invalid), (ErrorKind::Invalid, "account-invalid"));
    }
    assert_eq!(f.accounts.get(max.id()).await.unwrap(), max);
    let unknown = Id::from_uuid(uuid::Uuid::from_u128(9));
    let missing = f
        .accounts
        .update(unknown, AccountPatch::default(), Version::INITIAL)
        .await
        .unwrap_err();
    assert_eq!(code(&missing), (ErrorKind::NotFound, "account-not-found"));
}

#[tokio::test]
async fn an_update_that_changes_nothing_writes_nothing() {
    let f = fixture().await;
    let max = f.add("Max").await;
    let same = f
        .accounts
        .update(
            max.id(),
            AccountPatch {
                name: Some(String::from("Max")),
                enabled: Some(true),
                ..AccountPatch::default()
            },
            max.version(),
        )
        .await
        .unwrap();
    assert_eq!(same, max);
    assert_eq!(f.ops().await, [(Entity::Account, ChangeOp::Insert)]);
}

#[tokio::test]
async fn the_default_account_can_be_disabled_but_not_removed() {
    let f = fixture().await;
    let default = f.accounts.find("default").await.unwrap();
    let disabled = f
        .accounts
        .update(
            default.id(),
            AccountPatch {
                enabled: Some(false),
                ..AccountPatch::default()
            },
            default.version(),
        )
        .await
        .unwrap();
    assert!(!disabled.enabled());
    let refused = f
        .accounts
        .remove(default.id(), disabled.version(), false)
        .await
        .unwrap_err();
    assert_eq!(code(&refused), (ErrorKind::Invalid, "account-invalid"));
    assert_eq!(f.accounts.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn removing_deletes_the_row_the_token_and_on_request_the_folder() {
    let f = fixture().await;
    let max = f.add("Max").await;
    let with_token = f
        .accounts
        .set_token(max.id(), TOKEN, max.version())
        .await
        .unwrap();
    assert_eq!(f.secrets.keys().len(), 1);
    let stale = f
        .accounts
        .remove(max.id(), max.version(), true)
        .await
        .unwrap_err();
    assert_eq!(code(&stale), (ErrorKind::Conflict, "version-conflict"));
    assert!(f.folders.removed.lock().unwrap().is_empty());

    f.accounts
        .remove(max.id(), with_token.version(), true)
        .await
        .unwrap();
    assert_eq!(f.secrets.keys(), Vec::<String>::new());
    assert_eq!(*f.folders.removed.lock().unwrap(), ["/accounts/max"]);
    let gone = f.accounts.get(max.id()).await.unwrap_err();
    assert_eq!(code(&gone), (ErrorKind::NotFound, "account-not-found"));
    assert_eq!(
        f.ops().await,
        [
            (Entity::Account, ChangeOp::Insert),
            (Entity::Account, ChangeOp::Update),
            (Entity::Account, ChangeOp::Delete),
        ]
    );
}

#[tokio::test]
async fn folders_outside_the_accounts_folder_are_never_deleted() {
    let f = fixture().await;
    f.folders
        .existing
        .lock()
        .unwrap()
        .push(String::from("/profiles/work"));
    let work = f
        .accounts
        .create("Work", Some("/profiles/work"), None)
        .await
        .unwrap();
    let refused = f
        .accounts
        .remove(work.id(), work.version(), true)
        .await
        .unwrap_err();
    assert_eq!(code(&refused), (ErrorKind::Invalid, "account-invalid"));
    assert!(f.accounts.get(work.id()).await.is_ok());
    f.accounts
        .remove(work.id(), work.version(), false)
        .await
        .unwrap();
    assert!(f.folders.removed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn tokens_live_in_the_secret_store_and_the_row_keeps_a_reference() {
    let f = fixture().await;
    let max = f.add("Max").await;
    assert_eq!(f.accounts.run_token(max.id()).await.unwrap(), None);

    let set = f
        .accounts
        .set_token(max.id(), &format!("  {TOKEN}\n"), max.version())
        .await
        .unwrap();
    let reference = set.token_ref().unwrap().to_owned();
    assert_eq!(reference, format!("account-token/{}", max.id()));
    assert!(!reference.contains(TOKEN));
    assert_eq!(f.secrets.keys(), std::slice::from_ref(&reference));
    assert_eq!(
        f.secrets.get(&reference).await.unwrap().unwrap().expose(),
        TOKEN
    );
    assert_eq!(
        f.accounts
            .run_token(max.id())
            .await
            .unwrap()
            .unwrap()
            .expose(),
        TOKEN
    );
    assert_eq!(f.accounts.get(max.id()).await.unwrap(), set);
    assert_eq!(set.version(), max.version().next());

    let cleared = f
        .accounts
        .clear_token(max.id(), set.version())
        .await
        .unwrap();
    assert_eq!(cleared.token_ref(), None);
    assert_eq!(f.secrets.keys(), Vec::<String>::new());
    let again = f
        .accounts
        .clear_token(max.id(), cleared.version())
        .await
        .unwrap();
    assert_eq!(again, cleared);
}

#[tokio::test]
async fn replacing_a_token_keeps_the_version() {
    let f = fixture().await;
    let max = f.add("Max").await;
    let set = f
        .accounts
        .set_token(max.id(), TOKEN, max.version())
        .await
        .unwrap();
    let replaced = f
        .accounts
        .set_token(max.id(), "sk-ant-oat01-other", set.version())
        .await
        .unwrap();
    assert_eq!(replaced, set);
    let stored = f.accounts.run_token(max.id()).await.unwrap().unwrap();
    assert_eq!(stored.expose(), "sk-ant-oat01-other");
}

#[tokio::test]
async fn tokens_of_the_wrong_shape_are_refused_before_anything_is_stored() {
    let f = fixture().await;
    let max = f.add("Max").await;
    for bad in ["", "sk-ant-api03-key", "sk-ant-oat01 abc", "sk-ant-oat"] {
        let refused = f
            .accounts
            .set_token(max.id(), bad, max.version())
            .await
            .unwrap_err();
        assert_eq!(code(&refused), (ErrorKind::Invalid, "run-token-invalid"));
        assert!(!refused.message().contains("sk-ant-api03-key"));
    }
    let stale = f
        .accounts
        .set_token(max.id(), TOKEN, Version::from_stored(5))
        .await
        .unwrap_err();
    assert_eq!(code(&stale), (ErrorKind::Conflict, "version-conflict"));
    assert_eq!(f.secrets.keys(), Vec::<String>::new());
    assert_eq!(f.accounts.get(max.id()).await.unwrap(), max);
}

#[tokio::test]
async fn disabled_accounts_and_lost_tokens_are_unavailable_for_runs() {
    let f = fixture().await;
    let max = f.add("Max").await;
    let set = f
        .accounts
        .set_token(max.id(), TOKEN, max.version())
        .await
        .unwrap();
    f.secrets.delete(set.token_ref().unwrap()).await.unwrap();
    let lost = f.accounts.run_token(max.id()).await.unwrap_err();
    assert_eq!(code(&lost), (ErrorKind::Conflict, "account-unavailable"));
    let off = f
        .accounts
        .update(
            max.id(),
            AccountPatch {
                enabled: Some(false),
                ..AccountPatch::default()
            },
            set.version(),
        )
        .await
        .unwrap();
    assert!(!off.enabled());
    let disabled = f.accounts.run_token(max.id()).await.unwrap_err();
    assert_eq!(
        code(&disabled),
        (ErrorKind::Conflict, "account-unavailable")
    );
}

#[tokio::test]
async fn changes_name_the_account_entity_and_never_carry_secrets() {
    let f = fixture().await;
    let max = f.add("Max").await;
    f.accounts
        .set_token(max.id(), TOKEN, max.version())
        .await
        .unwrap();
    let changes = f.store.changes_since(0, 100).await.unwrap();
    assert_eq!(changes.len(), 2);
    for change in &changes {
        assert_eq!(change.entity, Entity::Account);
        assert_eq!(change.entity_id, max.id().as_uuid());
        assert!(!format!("{change:?}").contains(TOKEN));
    }
}

#[tokio::test]
async fn the_slug_is_unique_in_the_database() {
    let f = fixture().await;
    let max = f.add("Max").await;
    let twin = Account::new(
        Id::from_uuid(uuid::Uuid::now_v7()),
        max.slug().clone(),
        max.name().clone(),
        ConfigDir::from_stored(String::from("/accounts/twin")),
        9,
        max.created_at(),
    );
    let refused = nullradix_app::accounts::AccountRepository::insert(f.store.as_ref(), &twin)
        .await
        .unwrap_err();
    assert_eq!(code(&refused), (ErrorKind::Conflict, "account-exists"));
}

#[tokio::test]
async fn a_new_slug_never_gets_the_folder_of_another_account() {
    let f = fixture().await;
    f.folders
        .existing
        .lock()
        .unwrap()
        .push(String::from("/Accounts/BOB"));
    let work = f
        .accounts
        .create("Work", Some("/Accounts/BOB"), None)
        .await
        .unwrap();
    assert_eq!(work.config_dir().unwrap().as_str(), "/Accounts/BOB");
    let bob = f.add("Bob").await;
    assert_eq!(bob.slug().as_str(), "bob-2");
    assert_eq!(bob.config_dir().unwrap().as_str(), "/accounts/bob-2");
    let other = f.add("Bob").await;
    assert_eq!(other.slug().as_str(), "bob-3");
}

/// The store, with updates that can be made to fail.
struct Flaky {
    store: Arc<Store>,
    fail_updates: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl nullradix_app::accounts::AccountRepository for Flaky {
    async fn insert(&self, account: &Account) -> Result<nullradix_app::Change, AppError> {
        nullradix_app::accounts::AccountRepository::insert(self.store.as_ref(), account).await
    }

    async fn list(&self) -> Result<Vec<Account>, AppError> {
        nullradix_app::accounts::AccountRepository::list(self.store.as_ref()).await
    }

    async fn get(&self, id: Id<Account>) -> Result<Option<Account>, AppError> {
        nullradix_app::accounts::AccountRepository::get(self.store.as_ref(), id).await
    }

    async fn update(
        &self,
        account: &Account,
        expected: Version,
    ) -> Result<nullradix_app::Change, AppError> {
        if self.fail_updates.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AppError::version_conflict("account", account.id()));
        }
        nullradix_app::accounts::AccountRepository::update(self.store.as_ref(), account, expected)
            .await
    }

    async fn delete(
        &self,
        id: Id<Account>,
        expected: Version,
        at: nullradix_domain::Timestamp,
    ) -> Result<nullradix_app::Change, AppError> {
        nullradix_app::accounts::AccountRepository::delete(self.store.as_ref(), id, expected, at)
            .await
    }
}

async fn flaky() -> (Arc<AccountService>, Arc<Flaky>, Arc<MemorySecretStore>) {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let repository = Arc::new(Flaky {
        store,
        fail_updates: std::sync::atomic::AtomicBool::new(false),
    });
    let secrets = Arc::new(MemorySecretStore::new());
    let accounts = Arc::new(AccountService::new(
        repository.clone(),
        secrets.clone(),
        Arc::new(Folders::default()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
    ));
    (accounts, repository, secrets)
}

#[tokio::test]
async fn a_failed_update_leaves_no_new_token_and_keeps_the_old_one() {
    let (accounts, repository, secrets) = flaky().await;
    let max = accounts.create("Max", None, None).await.unwrap();
    let key = format!("account-token/{}", max.id());
    repository
        .fail_updates
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let first = accounts.set_token(max.id(), TOKEN, max.version()).await;
    assert!(first.is_err());
    assert_eq!(secrets.keys(), Vec::<String>::new());

    repository
        .fail_updates
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let set = accounts
        .set_token(max.id(), TOKEN, max.version())
        .await
        .unwrap();
    repository
        .fail_updates
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let kept = accounts.clear_token(max.id(), set.version()).await;
    assert!(kept.is_err());
    assert_eq!(secrets.get(&key).await.unwrap().unwrap().expose(), TOKEN);
}

#[tokio::test]
async fn a_stale_version_leaves_the_stored_token_alone() {
    let (accounts, _, secrets) = flaky().await;
    let max = accounts.create("Max", None, None).await.unwrap();
    let set = accounts
        .set_token(max.id(), TOKEN, max.version())
        .await
        .unwrap();
    let key = set.token_ref().unwrap().to_owned();
    let stale = accounts
        .set_token(max.id(), "sk-ant-oat01-other", max.version())
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    assert_eq!(secrets.get(&key).await.unwrap().unwrap().expose(), TOKEN);
}

#[tokio::test]
async fn racing_token_writes_leave_the_token_of_the_one_that_won() {
    let (accounts, _, secrets) = flaky().await;
    let max = accounts.create("Max", None, None).await.unwrap();
    let (id, version) = (max.id(), max.version());
    let first = tokio::spawn({
        let accounts = accounts.clone();
        async move { accounts.set_token(id, "sk-ant-oat01-first", version).await }
    });
    let second = tokio::spawn({
        let accounts = accounts.clone();
        async move { accounts.set_token(id, "sk-ant-oat01-second", version).await }
    });
    let results = [first.await.unwrap(), second.await.unwrap()];
    let winners: Vec<usize> = (0..2).filter(|at| results[*at].is_ok()).collect();
    assert_eq!(winners.len(), 1);
    let expected = ["sk-ant-oat01-first", "sk-ant-oat01-second"][winners[0]];
    let key = format!("account-token/{id}");
    assert_eq!(secrets.get(&key).await.unwrap().unwrap().expose(), expected);
}

#[tokio::test]
async fn a_removed_account_leaves_no_token_and_takes_no_new_one() {
    let (accounts, _, secrets) = flaky().await;
    let max = accounts.create("Max", None, None).await.unwrap();
    let set = accounts
        .set_token(max.id(), TOKEN, max.version())
        .await
        .unwrap();
    accounts
        .remove(max.id(), set.version(), false)
        .await
        .unwrap();
    assert_eq!(secrets.keys(), Vec::<String>::new());
    let late = accounts
        .set_token(max.id(), TOKEN, set.version())
        .await
        .unwrap_err();
    assert_eq!(late.code(), "account-not-found");
    assert_eq!(secrets.keys(), Vec::<String>::new());
}

#[tokio::test]
async fn a_folder_that_cannot_be_deleted_keeps_the_account() {
    let f = fixture().await;
    let max = f.add("Max").await;
    f.folders
        .fail_remove
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let refused = f.accounts.remove(max.id(), max.version(), true).await;
    assert!(refused.is_err());
    assert_eq!(f.accounts.get(max.id()).await.unwrap(), max);
    f.folders
        .fail_remove
        .store(false, std::sync::atomic::Ordering::SeqCst);
    f.accounts
        .remove(max.id(), max.version(), true)
        .await
        .unwrap();
    assert_eq!(*f.folders.removed.lock().unwrap(), ["/accounts/max"]);
}

#[tokio::test]
async fn a_folder_that_gains_a_file_during_removal_goes_on_the_second_try() {
    let f = fixture().await;
    let max = f.add("Max").await;
    f.folders
        .fail_once
        .store(true, std::sync::atomic::Ordering::SeqCst);
    f.accounts
        .remove(max.id(), max.version(), true)
        .await
        .unwrap();
    assert_eq!(*f.folders.removed.lock().unwrap(), ["/accounts/max"]);
}

/// Records when a folder is held and released around its removal.
struct Renewer {
    events: Arc<Mutex<Vec<&'static str>>>,
}

struct Release(Arc<Mutex<Vec<&'static str>>>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.lock().unwrap().push("released");
    }
}

#[async_trait]
impl nullradix_app::usage::LoginRenewal for Renewer {
    async fn renew(&self, _dir: Option<&ConfigDir>) -> nullradix_app::usage::Renewal {
        nullradix_app::usage::Renewal::Failed
    }

    async fn hold(&self, _dir: Option<&ConfigDir>) -> Box<dyn Send> {
        self.events.lock().unwrap().push("held");
        Box::new(Release(self.events.clone()))
    }
}

#[tokio::test]
async fn a_removal_holds_the_login_renewal_of_the_folder_until_it_is_done() {
    let f = fixture().await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let accounts = AccountService::new(
        f.store.clone(),
        f.secrets.clone(),
        f.folders.clone(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
    )
    .with_renewal(Arc::new(Renewer {
        events: events.clone(),
    }));
    let max = accounts.create("Max", None, None).await.unwrap();
    f.folders
        .fail_once
        .store(true, std::sync::atomic::Ordering::SeqCst);
    accounts
        .remove(max.id(), max.version(), true)
        .await
        .unwrap();
    assert_eq!(*f.folders.removed.lock().unwrap(), ["/accounts/max"]);
    assert_eq!(*events.lock().unwrap(), ["held", "released"]);
}

/// A secret store whose deletes can be made to fail.
#[derive(Default)]
struct StubbornSecrets {
    inner: MemorySecretStore,
    fail_deletes: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl SecretStore for StubbornSecrets {
    async fn get(
        &self,
        key: &str,
    ) -> Result<Option<nullradix_app::secrets::SecretValue>, AppError> {
        self.inner.get(key).await
    }

    async fn put(
        &self,
        key: &str,
        value: &nullradix_app::secrets::SecretValue,
    ) -> Result<(), AppError> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<(), AppError> {
        if self.fail_deletes.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(nullradix_app::secrets::secret_store_failed(
                std::io::Error::other("locked"),
            ));
        }
        self.inner.delete(key).await
    }
}

#[tokio::test]
async fn a_secret_store_that_cannot_delete_leaves_the_account_and_its_token_as_they_were() {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let secrets = Arc::new(StubbornSecrets::default());
    let accounts = AccountService::new(
        store,
        secrets.clone(),
        Arc::new(Folders::default()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
    );
    let max = accounts.create("Max", None, None).await.unwrap();
    let set = accounts
        .set_token(max.id(), TOKEN, max.version())
        .await
        .unwrap();
    secrets
        .fail_deletes
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let refused = accounts
        .clear_token(max.id(), set.version())
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "secret-store-failed");
    assert_eq!(accounts.get(max.id()).await.unwrap(), set);
    assert_eq!(secrets.inner.keys().len(), 1);

    secrets
        .fail_deletes
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let cleared = accounts.clear_token(max.id(), set.version()).await.unwrap();
    assert_eq!(cleared.token_ref(), None);
    assert_eq!(secrets.inner.keys(), Vec::<String>::new());
}

#[tokio::test]
async fn a_token_stored_without_a_row_reference_is_cleared_and_removed_too() {
    let f = fixture().await;
    let max = f.add("Max").await;
    let key = format!("account-token/{}", max.id());
    let stray = nullradix_app::secrets::SecretValue::new("sk-ant-oat01-stray");
    f.secrets.put(&key, &stray).await.unwrap();
    let cleared = f
        .accounts
        .clear_token(max.id(), max.version())
        .await
        .unwrap();
    assert_eq!(cleared, max);
    assert_eq!(f.secrets.keys(), Vec::<String>::new());

    f.secrets.put(&key, &stray).await.unwrap();
    f.accounts
        .remove(max.id(), max.version(), false)
        .await
        .unwrap();
    assert_eq!(f.secrets.keys(), Vec::<String>::new());
}

mod dispatch_accounts {
    use nullradix_app::dispatch::accounts::{
        AccountPause, AccountPauseRepository, NewSample, PauseReason, SamplePhase,
        UsageSampleRepository,
    };
    use nullradix_domain::{Timestamp, UsageSnapshot};

    use super::*;

    const AT: i64 = 1_800_000_000_000;
    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn pause(account: Id<Account>, until: i64, reason: PauseReason) -> AccountPause {
        AccountPause {
            account,
            until: Timestamp::from_millis(until),
            reason,
            run: None,
            created_at: Timestamp::from_millis(AT),
        }
    }

    fn poll(account: Id<Account>, five: i64, at: i64) -> NewSample {
        NewSample {
            id: uuid::Uuid::now_v7(),
            account,
            run: None,
            phase: SamplePhase::Poll,
            usage: UsageSnapshot {
                five_hour: Some(five),
                seven_day: Some(1),
                five_hour_resets_at: Some(Timestamp::from_millis(AT + DAY)),
                seven_day_resets_at: None,
            },
            at: Timestamp::from_millis(at),
        }
    }

    #[tokio::test]
    async fn a_pause_is_replaced_removed_and_reported_as_an_account_change() {
        let f = fixture().await;
        let max = f.add("Max").await;
        let store = f.store.as_ref();
        store
            .put(&pause(max.id(), AT + 1000, PauseReason::UsageLimit))
            .await
            .unwrap();
        store
            .put(&pause(max.id(), AT + 5000, PauseReason::Manual))
            .await
            .unwrap();
        let all = store.all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].until.as_millis(), AT + 5000);
        assert_eq!(all[0].reason, PauseReason::Manual);
        let removed = store
            .remove(max.id(), Timestamp::from_millis(AT))
            .await
            .unwrap();
        assert!(removed.is_some());
        assert!(
            store
                .remove(max.id(), Timestamp::from_millis(AT))
                .await
                .unwrap()
                .is_none()
        );
        let ops = f.ops().await;
        assert_eq!(
            ops.iter()
                .filter(|op| **op == (Entity::Account, ChangeOp::Update))
                .count(),
            3,
            "two puts and one remove: {ops:?}"
        );
    }

    #[tokio::test]
    async fn clearing_removes_every_pause_and_deleting_an_account_removes_its_pause() {
        let f = fixture().await;
        let max = f.add("Max").await;
        let pro = f.add("Pro").await;
        let store = f.store.as_ref();
        for account in [&max, &pro] {
            store
                .put(&pause(account.id(), AT + 1000, PauseReason::UsageLimit))
                .await
                .unwrap();
        }
        f.accounts
            .remove(pro.id(), pro.version(), false)
            .await
            .unwrap();
        assert_eq!(store.all().await.unwrap().len(), 1);
        let changes = store.clear(Timestamp::from_millis(AT)).await.unwrap();
        assert_eq!(changes.len(), 1);
        assert!(store.all().await.unwrap().is_empty());
        assert!(
            store
                .clear(Timestamp::from_millis(AT))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_poll_is_kept_only_when_the_numbers_changed() {
        let f = fixture().await;
        let max = f.add("Max").await;
        let store = f.store.as_ref();
        assert!(store.record_poll(&poll(max.id(), 10, AT)).await.unwrap());
        assert!(
            !store
                .record_poll(&poll(max.id(), 10, AT + 1000))
                .await
                .unwrap()
        );
        assert!(
            store
                .record_poll(&poll(max.id(), 11, AT + 2000))
                .await
                .unwrap()
        );
        // the other account has its own latest reading
        let pro = f.add("Pro").await;
        assert!(
            store
                .record_poll(&poll(pro.id(), 11, AT + 3000))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn polls_older_than_ninety_days_go_when_a_new_one_is_kept() {
        let f = fixture().await;
        let max = f.add("Max").await;
        let store = f.store.as_ref();
        store.record_poll(&poll(max.id(), 10, AT)).await.unwrap();
        // the old poll is removed, so its numbers are new again
        let later = AT + 91 * DAY;
        assert!(store.record_poll(&poll(max.id(), 12, later)).await.unwrap());
        assert!(
            store
                .record_poll(&poll(max.id(), 10, later + 1000))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn an_account_without_finished_runs_has_no_history_and_no_starts() {
        let f = fixture().await;
        let max = f.add("Max").await;
        let store = f.store.as_ref();
        store.record_poll(&poll(max.id(), 10, AT)).await.unwrap();
        assert!(store.history(500).await.unwrap().is_empty());
        let run = Id::from_uuid(uuid::Uuid::from_u128(5));
        assert!(store.starts(&[run]).await.unwrap().is_empty());
    }
}
