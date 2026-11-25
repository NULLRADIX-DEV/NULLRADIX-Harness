//! Claude accounts that runs are spread over, each with its own Claude folder and run token.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Account, AccountChanges, AccountError, AccountName, AccountSlug, ConfigDir, Id, Limit,
    RunToken, Timestamp, Version,
};
use parking_lot::Mutex;

use crate::secrets::{SecretStore, SecretValue};
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

#[async_trait]
pub trait AccountRepository: Send + Sync {
    /// Fails with [`slug_taken`] when another account uses the slug.
    async fn insert(&self, account: &Account) -> Result<Change, AppError>;
    /// In `position` order, the default account first.
    async fn list(&self) -> Result<Vec<Account>, AppError>;
    async fn get(&self, id: Id<Account>) -> Result<Option<Account>, AppError>;
    /// Stores `account` if the stored row still has version `expected`.
    async fn update(&self, account: &Account, expected: Version) -> Result<Change, AppError>;
    async fn delete(
        &self,
        id: Id<Account>,
        expected: Version,
        at: Timestamp,
    ) -> Result<Change, AppError>;
}

/// Claude's folders on the machine the daemon runs on.
#[async_trait]
pub trait AccountFolders: Send + Sync {
    /// Folder where new accounts get their own subfolders.
    fn root(&self) -> String;
    /// Creates the folder of a new account below [`root`](Self::root).
    async fn create(&self, slug: &AccountSlug) -> Result<ConfigDir, AppError>;
    /// Refuses a typed folder that is not absolute or does not exist.
    async fn check(&self, dir: &ConfigDir) -> Result<(), AppError>;
    /// Comparable spelling of a typed folder; equal keys mean the same folder.
    fn key(&self, dir: &ConfigDir) -> String;
    /// Comparable spelling of the folder a new account with this slug would get.
    fn slug_key(&self, slug: &AccountSlug) -> String;
    /// Whether the folder lies below [`root`](Self::root), so deleting it is allowed.
    fn owns(&self, dir: &ConfigDir) -> bool;
    /// Deletes the folder with everything in it.
    async fn remove(&self, dir: &ConfigDir) -> Result<(), AppError>;
}

/// Something that must end before an account is removed, such as its running login.
#[async_trait]
pub trait RemovalHook: Send + Sync {
    /// Called once every check passed, with the account's lock held.
    async fn removing(&self, account: &Account);
}

/// A removal with nothing to end first.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHook;

#[async_trait]
impl RemovalHook for NoHook {
    async fn removing(&self, _account: &Account) {}
}

fn invalid(error: &AccountError) -> AppError {
    match error {
        AccountError::Slug | AccountError::Limit(_) => {
            AppError::invalid("account-invalid", error.to_string())
        }
        AccountError::Name(text) | AccountError::ConfigDir(text) => {
            AppError::text(*text, "account-invalid", error.to_string())
        }
        AccountError::Token => AppError::invalid("run-token-invalid", error.to_string()),
    }
}

#[must_use]
pub fn not_found(id: impl Display) -> AppError {
    AppError::not_found("account-not-found", format!("account {id} does not exist"))
}

#[must_use]
pub fn slug_taken(slug: &AccountSlug) -> AppError {
    AppError::conflict(
        "account-exists",
        format!("another account already uses the id {slug}"),
    )
}

#[must_use]
pub fn default_stays() -> AppError {
    AppError::invalid(
        "account-invalid",
        "the default account can only be disabled, not removed",
    )
}

#[must_use]
pub fn dir_taken(dir: &ConfigDir, other: &AccountSlug) -> AppError {
    AppError::conflict(
        "account-dir-taken",
        format!(
            "the folder {} already belongs to account {other}",
            dir.as_str()
        ),
    )
}

#[must_use]
pub fn dir_not_owned(dir: &ConfigDir, root: &str) -> AppError {
    AppError::invalid(
        "account-invalid",
        format!(
            "the folder {} is not below {root}, delete it yourself",
            dir.as_str()
        ),
    )
}

#[must_use]
pub fn folder_relative(dir: &ConfigDir) -> AppError {
    AppError::invalid(
        "account-invalid",
        format!(
            "the folder {} must be an absolute path or start with ~",
            dir.as_str()
        ),
    )
}

#[must_use]
pub fn folder_missing(dir: &ConfigDir) -> AppError {
    AppError::invalid(
        "account-invalid",
        format!("the folder {} does not exist", dir.as_str()),
    )
}

#[must_use]
pub fn unavailable(slug: &AccountSlug, reason: &str) -> AppError {
    AppError::conflict(
        "account-unavailable",
        format!("account {slug} cannot be used: {reason}"),
    )
}

/// A change as a client sends it, before the values are checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountPatch {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub fallback: Option<bool>,
    #[allow(
        clippy::option_option,
        reason = "missing keeps the limit, none removes it"
    )]
    pub limit_five_hour: Option<Option<i64>>,
    #[allow(
        clippy::option_option,
        reason = "missing keeps the limit, none removes it"
    )]
    pub limit_week: Option<Option<i64>>,
}

impl AccountPatch {
    fn checked(self) -> Result<AccountChanges, AppError> {
        let limit = |value: Option<Option<i64>>, window| {
            value
                .map(|value| {
                    value
                        .map(|percent| Limit::parse(percent, window))
                        .transpose()
                })
                .transpose()
                .map_err(|error| invalid(&error))
        };
        Ok(AccountChanges {
            name: self
                .name
                .as_deref()
                .map(AccountName::parse)
                .transpose()
                .map_err(|error| invalid(&error))?,
            enabled: self.enabled,
            fallback: self.fallback,
            limit_five_hour: limit(self.limit_five_hour, "5 hours")?,
            limit_week: limit(self.limit_week, "the week")?,
        })
    }
}

pub struct AccountService {
    repository: Arc<dyn AccountRepository>,
    secrets: Arc<dyn SecretStore>,
    folders: Arc<dyn AccountFolders>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
    /// One lock per account, so token and row changes of an account never interleave.
    locks: Mutex<HashMap<Id<Account>, Arc<tokio::sync::Mutex<()>>>>,
}

impl std::fmt::Debug for AccountService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountService").finish_non_exhaustive()
    }
}

/// Key of an account's run token in the secret store.
fn token_key(id: Id<Account>) -> String {
    format!("account-token/{id}")
}

impl AccountService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn AccountRepository>,
        secrets: Arc<dyn SecretStore>,
        folders: Arc<dyn AccountFolders>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            repository,
            secrets,
            folders,
            clock,
            ids,
            signal,
            locks: Mutex::new(HashMap::new()),
        }
    }

    /// Folder where new accounts get their own subfolders.
    #[must_use]
    pub fn accounts_dir(&self) -> String {
        self.folders.root()
    }

    pub async fn list(&self) -> Result<Vec<Account>, AppError> {
        self.repository.list().await
    }

    pub async fn get(&self, id: Id<Account>) -> Result<Account, AppError> {
        self.repository.get(id).await?.ok_or_else(|| not_found(id))
    }

    /// The account with this slug.
    pub async fn find(&self, slug: &str) -> Result<Account, AppError> {
        self.repository
            .list()
            .await?
            .into_iter()
            .find(|account| account.slug().as_str() == slug)
            .ok_or_else(|| not_found(slug))
    }

    /// Adds an account; without a folder it gets its own below the accounts folder.
    pub async fn create(&self, name: &str, config_dir: Option<&str>) -> Result<Account, AppError> {
        let name = AccountName::parse(name).map_err(|error| invalid(&error))?;
        let given = config_dir
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
            .map(ConfigDir::parse)
            .transpose()
            .map_err(|error| invalid(&error))?;
        let existing = self.repository.list().await?;
        if let Some(dir) = &given {
            self.folders.check(dir).await?;
            let key = self.folders.key(dir);
            let other = existing.iter().find(|account| {
                account
                    .config_dir()
                    .is_some_and(|other| self.folders.key(other) == key)
            });
            if let Some(other) = other {
                return Err(dir_taken(dir, other.slug()));
            }
        }
        let own_folder = given.is_none();
        let slug = AccountSlug::from_name(name.as_str()).unique(|slug| {
            existing
                .iter()
                .any(|account| account.slug().as_str() == slug)
                || (own_folder && self.folder_taken(slug, &existing))
        });
        let dir = match given {
            Some(dir) => dir,
            None => self.folders.create(&slug).await?,
        };
        let position = existing
            .iter()
            .map(Account::position)
            .max()
            .map_or(0, |last| last.saturating_add(1));
        let account = Account::new(
            self.ids.as_ref().next(),
            slug,
            name,
            dir,
            position,
            self.clock.now(),
        );
        let change = self.repository.insert(&account).await?;
        self.signal.notify(&change);
        Ok(account)
    }

    /// Changes the account; a change that alters nothing returns it as it is.
    pub async fn update(
        &self,
        id: Id<Account>,
        patch: AccountPatch,
        expected: Version,
    ) -> Result<Account, AppError> {
        let changes = patch.checked()?;
        let _guard = self.lock(id).await;
        let current = self.current(id, expected).await?;
        let Some(next) = current.changed(changes, self.clock.now()) else {
            return Ok(current);
        };
        let change = self.repository.update(&next, expected).await?;
        self.signal.notify(&change);
        Ok(next)
    }

    /// Removes an account and its token; `delete_dir` also deletes its own folder.
    pub async fn remove(
        &self,
        id: Id<Account>,
        expected: Version,
        delete_dir: bool,
    ) -> Result<(), AppError> {
        self.remove_with(id, expected, delete_dir, &NoHook).await
    }

    /// Like [`remove`](Self::remove); `hook` runs after all refusals, before anything is deleted.
    pub async fn remove_with(
        &self,
        id: Id<Account>,
        expected: Version,
        delete_dir: bool,
        hook: &dyn RemovalHook,
    ) -> Result<(), AppError> {
        let _guard = self.lock(id).await;
        let current = self.current(id, expected).await?;
        if current.is_default() {
            return Err(default_stays());
        }
        let folder = current.config_dir().filter(|_| delete_dir);
        if let Some(dir) = folder
            && !self.folders.owns(dir)
        {
            return Err(dir_not_owned(dir, &self.folders.root()));
        }
        hook.removing(&current).await;
        // the folder goes first: if it cannot go, the account stays whole
        if let Some(dir) = folder {
            self.folders.remove(dir).await?;
        }
        let change = self
            .repository
            .delete(id, expected, self.clock.now())
            .await?;
        self.signal.notify(&change);
        for reference in Self::token_refs(id, &current) {
            if let Err(error) = self.secrets.delete(&reference).await {
                tracing::warn!(account = %current.slug(), code = error.code(), "token left in the secret store");
            }
        }
        self.locks.lock().remove(&id);
        Ok(())
    }

    /// Stores the long-lived token of `claude setup-token` for runs of this account.
    pub async fn set_token(
        &self,
        id: Id<Account>,
        token: &str,
        expected: Version,
    ) -> Result<Account, AppError> {
        let token = RunToken::parse(token).map_err(|error| invalid(&error))?;
        let _guard = self.lock(id).await;
        let current = self.current(id, expected).await?;
        let reference = token_key(id);
        let previous = self.secrets.get(&reference).await?;
        self.secrets
            .put(&reference, &SecretValue::new(token.expose()))
            .await?;
        let Some(next) = current.with_token_ref(Some(reference.clone()), self.clock.now()) else {
            return Ok(current);
        };
        match self.repository.update(&next, expected).await {
            Ok(change) => {
                self.signal.notify(&change);
                Ok(next)
            }
            Err(error) => {
                self.restore(&reference, previous).await;
                Err(error)
            }
        }
    }

    /// Forgets the token; runs use the login in the account's folder again.
    pub async fn clear_token(
        &self,
        id: Id<Account>,
        expected: Version,
    ) -> Result<Account, AppError> {
        let _guard = self.lock(id).await;
        let current = self.current(id, expected).await?;
        let key = token_key(id);
        let previous = self.secrets.get(&key).await?;
        // the secret goes first, so a failing store leaves the account as it was
        for reference in Self::token_refs(id, &current) {
            self.secrets.delete(&reference).await?;
        }
        let Some(next) = current.with_token_ref(None, self.clock.now()) else {
            return Ok(current);
        };
        match self.repository.update(&next, expected).await {
            Ok(change) => {
                self.signal.notify(&change);
                Ok(next)
            }
            Err(error) => {
                self.restore(&key, previous).await;
                Err(error)
            }
        }
    }

    /// The token runs of this account start with; none when the account has no token.
    pub async fn run_token(&self, id: Id<Account>) -> Result<Option<SecretValue>, AppError> {
        let account = self.get(id).await?;
        if !account.enabled() {
            return Err(unavailable(account.slug(), "it is disabled"));
        }
        let Some(reference) = account.token_ref() else {
            return Ok(None);
        };
        match self.secrets.get(reference).await? {
            Some(token) => Ok(Some(token)),
            None => Err(unavailable(
                account.slug(),
                "its token is missing from the secret store",
            )),
        }
    }

    /// Every key an account's token may sit under: the one by id, and the row's own reference.
    fn token_refs(id: Id<Account>, account: &Account) -> Vec<String> {
        let key = token_key(id);
        match account.token_ref() {
            Some(reference) if reference != key => vec![key, reference.to_owned()],
            _ => vec![key],
        }
    }

    /// Puts a secret back as it was before a change that then failed.
    async fn restore(&self, reference: &str, previous: Option<SecretValue>) {
        let undone = match previous {
            Some(value) => self.secrets.put(reference, &value).await,
            None => self.secrets.delete(reference).await,
        };
        if let Err(error) = undone {
            tracing::warn!(
                code = error.code(),
                "token not restored in the secret store"
            );
        }
    }

    async fn lock(&self, id: Id<Account>) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = Arc::clone(self.locks.lock().entry(id).or_default());
        lock.lock_owned().await
    }

    /// Whether the folder of a new account named `slug` is one that an account already uses.
    fn folder_taken(&self, slug: &str, existing: &[Account]) -> bool {
        let Ok(slug) = AccountSlug::parse(slug) else {
            return false;
        };
        let key = self.folders.slug_key(&slug);
        existing
            .iter()
            .filter_map(Account::config_dir)
            .any(|dir| self.folders.key(dir) == key)
    }

    async fn current(&self, id: Id<Account>, expected: Version) -> Result<Account, AppError> {
        let current = match self.get(id).await {
            Ok(current) => current,
            Err(error) => {
                if error.kind() == crate::ErrorKind::NotFound {
                    self.locks.lock().remove(&id);
                }
                return Err(error);
            }
        };
        if current.version() == expected {
            Ok(current)
        } else {
            Err(AppError::version_conflict("account", id))
        }
    }
}

#[cfg(test)]
mod tests {
    use nullradix_domain::TextError;

    use super::*;

    #[test]
    fn maps_validation_errors_to_stable_codes() {
        assert_eq!(invalid(&AccountError::Slug).code(), "account-invalid");
        assert_eq!(
            invalid(&AccountError::Name(TextError::Empty)).code(),
            "account-invalid"
        );
        assert_eq!(
            invalid(&AccountError::Name(TextError::TooLong(60))).code(),
            "value-too-long"
        );
        assert_eq!(
            invalid(&AccountError::ConfigDir(TextError::Empty)).code(),
            "account-invalid"
        );
        assert_eq!(
            invalid(&AccountError::Limit("week")).code(),
            "account-invalid"
        );
        assert_eq!(invalid(&AccountError::Token).code(), "run-token-invalid");
    }

    #[test]
    fn refusals_have_stable_codes_and_kinds() {
        let slug = AccountSlug::parse("max").unwrap();
        let dir = ConfigDir::parse("/a").unwrap();
        let cases = [
            (
                not_found("max"),
                "account-not-found",
                crate::ErrorKind::NotFound,
            ),
            (
                slug_taken(&slug),
                "account-exists",
                crate::ErrorKind::Conflict,
            ),
            (
                default_stays(),
                "account-invalid",
                crate::ErrorKind::Invalid,
            ),
            (
                dir_taken(&dir, &slug),
                "account-dir-taken",
                crate::ErrorKind::Conflict,
            ),
            (
                dir_not_owned(&dir, "/root"),
                "account-invalid",
                crate::ErrorKind::Invalid,
            ),
            (
                unavailable(&slug, "off"),
                "account-unavailable",
                crate::ErrorKind::Conflict,
            ),
        ];
        for (error, code, kind) in cases {
            assert_eq!((error.code(), error.kind()), (code, kind));
        }
    }

    #[test]
    fn patches_are_checked_before_anything_changes() {
        let patch = AccountPatch {
            limit_week: Some(Some(101)),
            ..AccountPatch::default()
        };
        assert_eq!(patch.checked().unwrap_err().code(), "account-invalid");
        let patch = AccountPatch {
            name: Some(String::from(" ")),
            ..AccountPatch::default()
        };
        assert_eq!(patch.checked().unwrap_err().code(), "account-invalid");
        let patch = AccountPatch {
            limit_five_hour: Some(None),
            limit_week: Some(Some(80)),
            ..AccountPatch::default()
        };
        let changes = patch.checked().unwrap();
        assert_eq!(changes.limit_five_hour, Some(None));
        assert_eq!(changes.limit_week.flatten().map(Limit::percent), Some(80));
    }

    #[test]
    fn token_keys_are_built_from_the_account_id() {
        let id = Id::from_uuid(uuid::Uuid::from_u128(7));
        assert_eq!(token_key(id), format!("account-token/{id}"));
    }
}
