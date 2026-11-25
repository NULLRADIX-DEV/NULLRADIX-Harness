//! The configuration of the dispatcher: settings, profiles, rules, ladder and its versions.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{ConfigError, DispatcherConfig, Fingerprint, Timestamp, Version};
use uuid::Uuid;

use crate::accounts::AccountRepository;
use crate::{AppError, Change, ChangeSignal, Clock, FieldProblem, IdGenerator};

/// Entity id of the one dispatcher configuration in the change feed.
pub const CONFIG_ENTITY_ID: Uuid = Uuid::from_u128(0x019a_0000_0000_7000_8000_0000_0000_0002);

/// The stored configuration with what the store keeps beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConfig {
    pub config: DispatcherConfig,
    /// Hash the store last recorded; it may lag behind after accounts were removed.
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

#[async_trait]
pub trait DispatcherConfigRepository: Send + Sync {
    /// The stored configuration; a fresh store holds the defaults.
    async fn load(&self) -> Result<StoredConfig, AppError>;
    /// Stores the configuration, bumps its version and keeps the hash as a version.
    async fn save(&self, save: &ConfigSave<'_>) -> Result<Change, AppError>;
    /// Keeps `fingerprint` as the hash of the stored configuration without changing the version.
    async fn record_hash(&self, fingerprint: &Fingerprint, at: Timestamp) -> Result<(), AppError>;
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

    /// The configuration in force; its hash is kept as a version when it is new.
    pub async fn get(&self) -> Result<ConfigView, AppError> {
        let stored = self.repository.load().await?;
        let fingerprint = stored.config.fingerprint();
        if fingerprint.hash != stored.hash {
            self.repository
                .record_hash(&fingerprint, self.clock.now())
                .await?;
        }
        Ok(ConfigView {
            config: stored.config,
            hash: fingerprint.hash,
            version: stored.version,
            updated_at: stored.updated_at,
        })
    }

    /// Replaces the configuration; a draft that changes nothing returns the current one.
    pub async fn update(
        &self,
        draft: DispatcherConfig,
        expected: Version,
    ) -> Result<ConfigView, AppError> {
        let config = draft.normalized().map_err(issues)?;
        self.check_accounts(&config).await?;
        let current = self.get().await?;
        if current.version != expected {
            return Err(AppError::conflict(
                "version-conflict",
                "the dispatcher configuration was changed by someone else, reload and try again",
            ));
        }
        if config == current.config {
            return Ok(current);
        }
        let fingerprint = config.fingerprint();
        let at = self.clock.now();
        let change = self
            .repository
            .save(&ConfigSave {
                config: &config,
                fingerprint: &fingerprint,
                expected,
                at,
                ids: self.ids.as_ref(),
            })
            .await?;
        self.signal.notify(&change);
        Ok(ConfigView {
            config,
            hash: fingerprint.hash,
            version: expected.next(),
            updated_at: at,
        })
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
