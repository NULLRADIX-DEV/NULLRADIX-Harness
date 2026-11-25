#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_app::accounts::{AccountFolders, AccountPatch, AccountService};
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

    fn owns(&self, dir: &ConfigDir) -> bool {
        dir.as_str().starts_with("/accounts/")
    }

    async fn remove(&self, dir: &ConfigDir) -> Result<(), AppError> {
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
        self.accounts.create(name, None).await.unwrap()
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
        .create("Work", Some(" /profiles/work "))
        .await
        .unwrap();
    assert_eq!(work.config_dir().unwrap().as_str(), "/profiles/work");

    let taken = f
        .accounts
        .create("Other", Some("/PROFILES/work"))
        .await
        .unwrap_err();
    assert_eq!(code(&taken), (ErrorKind::Conflict, "account-dir-taken"));
    let relative = f.accounts.create("Rel", Some("profiles/work")).await;
    assert_eq!(
        code(&relative.unwrap_err()),
        (ErrorKind::Invalid, "account-invalid")
    );
    let missing = f.accounts.create("Gone", Some("/profiles/gone")).await;
    assert_eq!(
        code(&missing.unwrap_err()),
        (ErrorKind::Invalid, "account-invalid")
    );
    assert_eq!(f.accounts.list().await.unwrap().len(), 2);
}

#[tokio::test]
async fn names_are_validated() {
    let f = fixture().await;
    let blank = f.accounts.create("  ", None).await.unwrap_err();
    assert_eq!(code(&blank), (ErrorKind::Invalid, "account-invalid"));
    let long = f.accounts.create(&"x".repeat(61), None).await.unwrap_err();
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
        .create("Work", Some("/profiles/work"))
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
