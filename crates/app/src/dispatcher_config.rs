//! The configuration of the dispatcher: settings, profiles, rules, ladder and its versions.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{ConfigError, DispatcherConfig, Fingerprint, Timestamp, Version};
use uuid::Uuid;

use crate::accounts::AccountRepository;
use crate::auth::CommandRefusal;
use crate::{AppError, Change, ChangeSignal, Clock, FieldProblem, IdGenerator};

/// Entity id of the one dispatcher configuration in the change feed.
pub const CONFIG_ENTITY_ID: Uuid = Uuid::from_u128(0x019a_0000_0000_7000_8000_0000_0000_0002);

/// The stored configuration with what the store keeps beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConfig {
    pub config: DispatcherConfig,
    /// Hash the store last recorded; it lags until `sync_hash` runs.
    pub hash: String,
    pub version: Version,
    pub updated_at: Timestamp,
}

/// A configuration as it was in force once, named by its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigVersion {
    pub hash: String,
    pub content: String,
    pub first_seen_at: Timestamp,
}

/// What to store when the configuration changes.
#[derive(Clone, Copy)]
pub struct ConfigSave<'a> {
    pub config: &'a DispatcherConfig,
    pub fingerprint: &'a Fingerprint,
    pub expected: Version,
    pub at: Timestamp,
    /// New rows such as added profiles take their ids from here.
    pub ids: &'a dyn IdGenerator,
}

/// What a save did; the refusals are the caller's to name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved(Change),
    /// The stored version is no longer the expected one.
    Stale,
    /// A profile or rule names an account that is gone.
    UnknownAccount(FieldProblem),
}

#[async_trait]
pub trait DispatcherConfigRepository: Send + Sync {
    /// The stored configuration; a fresh store holds the defaults.
    async fn load(&self) -> Result<StoredConfig, AppError>;
    /// Stores the configuration, bumps its version and keeps the hash as a version.
    async fn save(&self, save: &ConfigSave<'_>) -> Result<SaveOutcome, AppError>;
    /// Keeps `fingerprint` as the hash of the stored configuration while it is still at `expected`.
    async fn record_hash(
        &self,
        fingerprint: &Fingerprint,
        expected: Version,
        at: Timestamp,
    ) -> Result<(), AppError>;
    /// Every configuration that was in force, oldest first.
    async fn versions(&self) -> Result<Vec<ConfigVersion>, AppError>;
}

/// The configuration as clients see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigView {
    pub config: DispatcherConfig,
    pub hash: String,
    pub version: Version,
    pub updated_at: Timestamp,
}

fn invalid_fields(fields: Vec<FieldProblem>) -> AppError {
    AppError::invalid(
        "dispatcher-config-invalid",
        "the dispatcher configuration has invalid fields",
    )
    .with_fields(fields)
}

fn stale_version() -> AppError {
    AppError::conflict(
        "version-conflict",
        "the dispatcher configuration was changed by someone else, reload and try again",
    )
}

fn unknown_account(problem: FieldProblem) -> AppError {
    invalid_fields(vec![problem])
}

fn issues(error: ConfigError) -> AppError {
    invalid_fields(
        error
            .0
            .into_iter()
            .map(|found| FieldProblem {
                field: found.field,
                message: found.message,
            })
            .collect(),
    )
}

pub struct DispatcherConfigService {
    repository: Arc<dyn DispatcherConfigRepository>,
    accounts: Arc<dyn AccountRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for DispatcherConfigService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatcherConfigService")
            .finish_non_exhaustive()
    }
}

impl DispatcherConfigService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn DispatcherConfigRepository>,
        accounts: Arc<dyn AccountRepository>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            repository,
            accounts,
            clock,
            ids,
            signal,
        }
    }

    /// The configuration in force; reading never writes.
    pub async fn get(&self) -> Result<ConfigView, AppError> {
        let stored = self.repository.load().await?;
        let hash = stored.config.fingerprint().hash;
        Ok(ConfigView {
            config: stored.config,
            hash,
            version: stored.version,
            updated_at: stored.updated_at,
        })
    }

    /// Keeps the hash of the stored configuration when it differs from what the store recorded.
    pub async fn sync_hash(&self) -> Result<(), AppError> {
        let stored = self.repository.load().await?;
        let fingerprint = stored.config.fingerprint();
        if fingerprint.hash != stored.hash {
            self.repository
                .record_hash(&fingerprint, stored.version, self.clock.now())
                .await?;
        }
        Ok(())
    }

    /// Replaces the configuration, and `commands` says why command fields may not change.
    pub async fn update(
        &self,
        draft: DispatcherConfig,
        expected: Version,
        commands: Option<CommandRefusal>,
    ) -> Result<ConfigView, AppError> {
        let config = draft.normalized().map_err(issues)?;
        self.check_accounts(&config).await?;
        let current = self.get().await?;
        if current.version != expected {
            return Err(stale_version());
        }
        if config.clone().without_ids() == current.config.clone().without_ids() {
            return Ok(current);
        }
        if let Some(refusal) = commands {
            let changed = current.config.command_fields_changed(&config);
            if !changed.is_empty() {
                return Err(refusal.error(&changed));
            }
        }
        let fingerprint = config.fingerprint();
        let at = self.clock.now();
        let outcome = self
            .repository
            .save(&ConfigSave {
                config: &config,
                fingerprint: &fingerprint,
                expected,
                at,
                ids: self.ids.as_ref(),
            })
            .await?;
        match outcome {
            SaveOutcome::Saved(change) => self.signal.notify(&change),
            SaveOutcome::Stale => return Err(stale_version()),
            SaveOutcome::UnknownAccount(problem) => return Err(unknown_account(problem)),
        }
        self.get().await
    }

    /// Every configuration that was in force, oldest first.
    pub async fn versions(&self) -> Result<Vec<ConfigVersion>, AppError> {
        self.repository.versions().await
    }

    async fn check_accounts(&self, config: &DispatcherConfig) -> Result<(), AppError> {
        let references = config.account_references();
        if references.is_empty() {
            return Ok(());
        }
        let known = self.accounts.list().await?;
        let fields: Vec<FieldProblem> = references
            .into_iter()
            .filter(|(_, slug)| !known.iter().any(|account| account.slug().as_str() == *slug))
            .map(|(field, slug)| FieldProblem {
                field,
                message: format!("there is no account with the id \"{slug}\""),
            })
            .collect();
        if fields.is_empty() {
            return Ok(());
        }
        Err(invalid_fields(fields))
    }
}
