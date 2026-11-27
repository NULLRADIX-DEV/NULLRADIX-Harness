//! Themes: installed packages, the chosen theme and the files a theme serves.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    BUILTIN_THEMES, DEFAULT_THEME, InstalledTheme, RiskAdded, SettingValue, SourceId, ThemeId,
    ThemeManifest, ThemeRisk, ThemeSettingValues, Timestamp, Version, asset_segments, content_type,
    inspect_package,
};
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::theme_package::{PackageFile, check_package};
use crate::theme_settings::settings_invalid;
use crate::{AppError, Change, ChangeSignal, Clock, ConfirmationFacts, IdGenerator};

/// Entity id of the chosen theme in the change feed, which only takes UUIDs.
pub const SELECTION_FEED_ID: Uuid = Uuid::from_u128(0x6e75_6c6c_7261_6469_7800_7468_656d_6500);

/// A file of an installed theme with the type it is served as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub data: Vec<u8>,
    pub content_type: &'static str,
}

/// A theme that was installed and what its package added to the version it replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub theme: InstalledTheme,
    /// Code, domains and Claude part the replaced version lacked; the code switch turns off on any.
    pub added: RiskAdded,
}

/// The package needs a confirmation: the facts go with the refusal.
fn confirmation_required(
    manifest: &ThemeManifest,
    risk: ThemeRisk,
    added: RiskAdded,
    update: bool,
) -> AppError {
    let mut parts = Vec::new();
    if risk.has_code {
        parts.push(String::from("code"));
    }
    if risk.claude_theme {
        parts.push(String::from("a Claude part"));
    }
    if !risk.network.is_empty() {
        parts.push(format!("network domains {}", risk.network.join(", ")));
    }
    AppError::conflict(
        "theme-confirmation-required",
        format!("{} contains {}", manifest.id(), parts.join(" and ")),
    )
    .with_confirmation(ConfirmationFacts {
        risk,
        added,
        update,
        version: manifest.record().version.clone(),
        sha256: None,
    })
}

/// The installed themes and which one is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeList {
    pub themes: Vec<InstalledTheme>,
    /// A built-in theme or the id of an installed one.
    pub active: String,
}

/// The values stored for a theme and the version of their row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSettings {
    pub values: ThemeSettingValues,
    pub version: Version,
}

/// The values a theme runs with; `version` is `None` while nothing was saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeSettings {
    pub values: ThemeSettingValues,
    pub version: Option<Version>,
}

impl From<Option<StoredSettings>> for ThemeSettings {
    fn from(stored: Option<StoredSettings>) -> Self {
        match stored {
            Some(stored) => Self {
                values: stored.values,
                version: Some(stored.version),
            },
            None => Self {
                values: ThemeSettingValues::default(),
                version: None,
            },
        }
    }
}

/// One write; dropping it without commit rolls back.
#[async_trait]
pub trait ThemeWrite: Send {
    async fn theme(&mut self, id: &ThemeId) -> Result<Option<InstalledTheme>, AppError>;
    async fn insert(&mut self, theme: &InstalledTheme, feed_id: Uuid) -> Result<Change, AppError>;
    /// Fails with a version conflict when the stored theme is not at `previous`.
    async fn update(
        &mut self,
        theme: &InstalledTheme,
        previous: Version,
    ) -> Result<Change, AppError>;
    /// Removes the theme; `None` when it was not installed.
    async fn delete(
        &mut self,
        id: &ThemeId,
        expected: Version,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    async fn selection(&mut self) -> Result<Option<String>, AppError>;
    /// The change is `None` when the choice stays as it is.
    async fn select(&mut self, id: &str, at: Timestamp) -> Result<Option<Change>, AppError>;
    async fn settings(&mut self, id: &ThemeId) -> Result<Option<StoredSettings>, AppError>;
    /// Stores the values when the row is at `expected` (`None`: no row yet), else a conflict.
    async fn put_settings(
        &mut self,
        id: &ThemeId,
        values: &ThemeSettingValues,
        expected: Option<Version>,
        at: Timestamp,
    ) -> Result<(Version, Change), AppError>;
    /// Drops the values when the row is at `expected`; `None` when nothing was stored.
    async fn delete_settings(
        &mut self,
        id: &ThemeId,
        expected: Version,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    /// Drops the values whatever their version; `None` when nothing was stored.
    async fn purge_settings(
        &mut self,
        id: &ThemeId,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    /// Records the catalog source the theme came from; `None` forgets it.
    async fn set_origin(
        &mut self,
        id: &ThemeId,
        source: Option<&SourceId>,
        at: Timestamp,
    ) -> Result<(), AppError>;
    async fn commit(self: Box<Self>) -> Result<(), AppError>;
}

#[async_trait]
pub trait ThemeRepository: Send + Sync {
    async fn begin(&self) -> Result<Box<dyn ThemeWrite>, AppError>;
    /// Installed themes sorted by id.
    async fn list(&self) -> Result<Vec<InstalledTheme>, AppError>;
    async fn get(&self, id: &ThemeId) -> Result<Option<InstalledTheme>, AppError>;
    /// The chosen theme; `None` while none was chosen.
    async fn selection(&self) -> Result<Option<String>, AppError>;
    async fn settings(&self, id: &ThemeId) -> Result<Option<StoredSettings>, AppError>;
    /// The catalog source of every theme installed from one.
    async fn origins(&self) -> Result<BTreeMap<ThemeId, SourceId>, AppError>;
}

/// Where an install comes from and whether it may take over a theme from elsewhere.
#[derive(Debug, Clone, Copy)]
pub struct Origin<'a> {
    pub source: &'a SourceId,
    pub replace: bool,
}

/// The files of installed themes; adapters report refused packages with `package_invalid`.
#[async_trait]
pub trait ThemeFiles: Send + Sync {
    /// The files of a package archive; refuses links, escaping paths and oversized content.
    async fn unpack(&self, archive: Vec<u8>) -> Result<Vec<PackageFile>, AppError>;
    /// Replaces the folder of the theme with `files`, all or nothing.
    async fn store(&self, id: &ThemeId, files: Vec<PackageFile>) -> Result<(), AppError>;
    async fn remove(&self, id: &ThemeId) -> Result<(), AppError>;
    /// The file at `path` below the theme's folder; `None` unless it is a regular file inside it.
    async fn read(&self, id: &ThemeId, path: &[String]) -> Result<Option<Vec<u8>>, AppError>;
    /// The paths of the files of the theme; empty when its folder is missing.
    async fn paths(&self, id: &ThemeId) -> Result<Vec<String>, AppError>;
}

#[must_use]
pub fn theme_not_found(id: &str) -> AppError {
    AppError::not_found(
        "theme-not-found",
        format!("the theme {id} is not installed"),
    )
}

fn id_invalid(error: &nullradix_domain::ThemeError) -> AppError {
    AppError::invalid("theme-id-invalid", error.to_string())
}

/// The folder of the themes cannot be read or written; the cause stays in the log.
#[must_use]
pub fn files_failed(error: impl std::error::Error + Send + Sync + 'static) -> AppError {
    AppError::unavailable("theme-files-failed", "cannot read or write the theme files")
        .with_source(error)
}

/// Theme files for hosts that have no data folder: nothing can be installed or served.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableFiles;

#[async_trait]
impl ThemeFiles for UnavailableFiles {
    async fn unpack(&self, _archive: Vec<u8>) -> Result<Vec<PackageFile>, AppError> {
        Err(unavailable())
    }

    async fn store(&self, _id: &ThemeId, _files: Vec<PackageFile>) -> Result<(), AppError> {
        Err(unavailable())
    }

    async fn remove(&self, _id: &ThemeId) -> Result<(), AppError> {
        Ok(())
    }

    async fn read(&self, _id: &ThemeId, _path: &[String]) -> Result<Option<Vec<u8>>, AppError> {
        Ok(None)
    }

    async fn paths(&self, _id: &ThemeId) -> Result<Vec<String>, AppError> {
        Ok(Vec::new())
    }
}

fn unavailable() -> AppError {
    AppError::unavailable("theme-files-failed", "this host keeps no theme files")
}

fn file_not_found() -> AppError {
    AppError::not_found("theme-file-not-found", "no such theme file")
}

pub struct ThemeService {
    repository: Arc<dyn ThemeRepository>,
    files: Arc<dyn ThemeFiles>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
    /// One install or removal at a time, so files and rows never disagree.
    writes: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for ThemeService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThemeService").finish_non_exhaustive()
    }
}

impl ThemeService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn ThemeRepository>,
        files: Arc<dyn ThemeFiles>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            repository,
            files,
            clock,
            ids,
            signal,
            writes: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn list(&self) -> Result<ThemeList, AppError> {
        let themes = self.repository.list().await?;
        let selection = self.repository.selection().await?;
        // a stored choice whose package is gone shows as the default, as legacy did
        let active = selection
            .filter(|id| {
                BUILTIN_THEMES.contains(&id.as_str())
                    || themes.iter().any(|theme| theme.id().as_str() == id)
            })
            .unwrap_or_else(|| String::from(DEFAULT_THEME));
        Ok(ThemeList { themes, active })
    }

    /// Installs or replaces the theme; new code or domains need `confirmed`.
    pub async fn install(&self, archive: Vec<u8>, confirmed: bool) -> Result<Installed, AppError> {
        self.install_expecting(archive, None, None, confirmed).await
    }

    /// Installs a package that must be the theme and version expected, recording its origin.
    pub async fn install_expecting(
        &self,
        archive: Vec<u8>,
        expected: Option<(&ThemeId, &str)>,
        origin: Option<Origin<'_>>,
        confirmed: bool,
    ) -> Result<Installed, AppError> {
        let files = self.files.unpack(archive).await?;
        let manifest = check_package(&files, false)?;
        if let Some((id, version)) = expected
            && (manifest.id() != id || manifest.record().version != version)
        {
            return Err(AppError::invalid(
                "theme-package-mismatch",
                format!(
                    "the package holds {} {}, not {id} {version}",
                    manifest.id(),
                    manifest.record().version
                ),
            ));
        }
        if manifest.id().is_builtin() {
            return Err(AppError::invalid(
                "theme-id-reserved",
                format!("{} is the name of a built-in theme", manifest.id()),
            ));
        }
        let _writing = self.writes.lock().await;
        if let Some(origin) = &origin
            && !origin.replace
            && self.repository.get(manifest.id()).await?.is_some()
            && self.repository.origins().await?.get(manifest.id()) != Some(origin.source)
        {
            return Err(AppError::conflict(
                "catalog-theme-installed-elsewhere",
                format!(
                    "{} is installed from another source or by upload; replace it explicitly",
                    manifest.id()
                ),
            ));
        }
        let known = self.repository.get(manifest.id()).await?;
        let (risk, added) = self.inspect(&manifest, &files, known.as_ref()).await?;
        if !confirmed && risk.is_risky() {
            return Err(confirmation_required(
                &manifest,
                risk,
                added,
                known.is_some(),
            ));
        }
        self.files.store(manifest.id(), files).await?;
        let now = self.clock.now();
        let mut write = self.repository.begin().await?;
        let (theme, change) = if let Some(known) = write.theme(manifest.id()).await? {
            let next = known.reinstalled(manifest, now);
            let change = write.update(&next, known.record().version).await?;
            (next, change)
        } else {
            let theme = InstalledTheme::install(manifest, now);
            let change = write.insert(&theme, self.ids.next_uuid()).await?;
            (theme, change)
        };
        write
            .set_origin(theme.id(), origin.map(|origin| origin.source), now)
            .await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(Installed { theme, added })
    }

    /// The risk of the package and what it brings that the installed version did not have.
    async fn inspect(
        &self,
        manifest: &ThemeManifest,
        files: &[PackageFile],
        known: Option<&InstalledTheme>,
    ) -> Result<(ThemeRisk, RiskAdded), AppError> {
        let risk = inspect_package(
            manifest.record(),
            files.iter().map(|file| file.path.as_str()),
        );
        let Some(known) = known else {
            let added = risk.added_to(None);
            return Ok((risk, added));
        };
        let paths = self.files.paths(known.id()).await?;
        let old = inspect_package(
            known.record().manifest.record(),
            paths.iter().map(String::as_str),
        );
        let added = risk.added_to(Some(&old));
        Ok((risk, added))
    }

    /// The catalog source of every theme installed from one.
    pub async fn origins(&self) -> Result<BTreeMap<ThemeId, SourceId>, AppError> {
        self.repository.origins().await
    }

    /// Chooses a built-in or an installed theme; the choice survives restarts.
    pub async fn select(&self, id: &str) -> Result<String, AppError> {
        if !BUILTIN_THEMES.contains(&id) {
            let theme = ThemeId::parse(id).map_err(|error| id_invalid(&error))?;
            if self.repository.get(&theme).await?.is_none() {
                return Err(theme_not_found(id));
            }
        }
        let mut write = self.repository.begin().await?;
        let change = write.select(id, self.clock.now()).await?;
        write.commit().await?;
        if let Some(change) = &change {
            self.signal.notify(change);
        }
        Ok(id.to_owned())
    }

    /// Uninstalls the theme; when it was chosen the default takes over again.
    pub async fn remove(&self, id: &str, expected: Version) -> Result<(), AppError> {
        let theme = ThemeId::parse(id).map_err(|error| id_invalid(&error))?;
        let _writing = self.writes.lock().await;
        let now = self.clock.now();
        let mut write = self.repository.begin().await?;
        // the values go with the theme, in the same transaction
        let purged = write.purge_settings(&theme, now).await?;
        write.set_origin(&theme, None, now).await?;
        let Some(change) = write.delete(&theme, expected, now).await? else {
            return Err(theme_not_found(id));
        };
        let mut changes = vec![change];
        changes.extend(purged);
        if write.selection().await?.as_deref() == Some(id) {
            changes.extend(write.select(DEFAULT_THEME, now).await?);
        }
        write.commit().await?;
        for change in &changes {
            self.signal.notify(change);
        }
        // the row is gone; leftover files are inert and a later install replaces them
        if let Err(error) = self.files.remove(&theme).await {
            tracing::warn!(%error, theme = id, "cannot remove the files of the theme");
        }
        Ok(())
    }

    /// The values saved for an installed theme; none until something was saved.
    pub async fn settings(&self, id: &str) -> Result<ThemeSettings, AppError> {
        let theme = ThemeId::parse(id).map_err(|error| id_invalid(&error))?;
        let installed = self
            .repository
            .get(&theme)
            .await?
            .ok_or_else(|| theme_not_found(id))?;
        let specs = installed.record().manifest.record().settings.as_ref();
        let mut settings: ThemeSettings = self.repository.settings(&theme).await?.into();
        settings.values = settings.values.fitting(specs);
        Ok(settings)
    }

    /// Replaces the saved values after checking them against the settings the theme declares.
    pub async fn save_settings(
        &self,
        id: &str,
        input: BTreeMap<String, SettingValue>,
        expected: Option<Version>,
    ) -> Result<ThemeSettings, AppError> {
        let theme_id = ThemeId::parse(id).map_err(|error| id_invalid(&error))?;
        let mut write = self.repository.begin().await?;
        // the manifest and the row are read in one transaction, so a reinstall cannot slip in
        let theme = write
            .theme(&theme_id)
            .await?
            .ok_or_else(|| theme_not_found(id))?;
        let specs = theme.record().manifest.record().settings.as_ref();
        let values = ThemeSettingValues::check(specs, input).map_err(settings_invalid)?;
        let (version, change) = write
            .put_settings(&theme_id, &values, expected, self.clock.now())
            .await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(ThemeSettings {
            values,
            version: Some(version),
        })
    }

    /// Back to the defaults of the manifest; nothing saved is already reset.
    pub async fn reset_settings(
        &self,
        id: &str,
        expected: Option<Version>,
    ) -> Result<(), AppError> {
        let theme = ThemeId::parse(id).map_err(|error| id_invalid(&error))?;
        let mut write = self.repository.begin().await?;
        if write.theme(&theme).await?.is_none() {
            return Err(theme_not_found(id));
        }
        let change = match (write.settings(&theme).await?, expected) {
            (None, _) => None,
            (Some(stored), Some(expected)) if stored.version == expected => {
                write
                    .delete_settings(&theme, expected, self.clock.now())
                    .await?
            }
            (Some(_), _) => return Err(AppError::version_conflict("theme settings", id)),
        };
        write.commit().await?;
        if let Some(change) = &change {
            self.signal.notify(change);
        }
        Ok(())
    }

    /// A file of an installed theme; anything unsafe or missing is simply not found.
    pub async fn asset(&self, id: &str, path: &str) -> Result<Asset, AppError> {
        let theme = ThemeId::parse(id).map_err(|_| file_not_found())?;
        let segments = asset_segments(path).map_err(|_| file_not_found())?;
        let segments: Vec<String> = segments.into_iter().map(str::to_owned).collect();
        // files of a removed theme may linger on disk; they are not served
        self.repository
            .get(&theme)
            .await?
            .ok_or_else(file_not_found)?;
        let data = self
            .files
            .read(&theme, &segments)
            .await?
            .ok_or_else(file_not_found)?;
        Ok(Asset {
            data,
            content_type: content_type(path),
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;
    use crate::SystemClock;
    use crate::UuidV7Generator;
    use crate::theme_package::tests::{MANIFEST, file, package};
    use crate::{ChangeOp, Entity};

    #[derive(Default)]
    pub(crate) struct Memory {
        themes: Mutex<BTreeMap<String, (InstalledTheme, Uuid)>>,
        selection: Mutex<Option<String>>,
        feed: Mutex<Vec<(Entity, Uuid, ChangeOp)>>,
        settings: Mutex<BTreeMap<String, StoredSettings>>,
        origins: Mutex<BTreeMap<String, SourceId>>,
    }

    impl Memory {
        fn record(&self, entity: Entity, id: Uuid, op: ChangeOp) -> Change {
            let mut feed = self.feed.lock();
            feed.push((entity, id, op));
            Change {
                seq: i64::try_from(feed.len()).unwrap(),
                entity,
                entity_id: id,
                op,
                at: Timestamp::from_millis(0),
            }
        }
    }

    struct Write(Arc<Memory>);

    #[async_trait]
    impl ThemeWrite for Write {
        async fn theme(&mut self, id: &ThemeId) -> Result<Option<InstalledTheme>, AppError> {
            Ok(self
                .0
                .themes
                .lock()
                .get(id.as_str())
                .map(|(theme, _)| theme.clone()))
        }

        async fn insert(
            &mut self,
            theme: &InstalledTheme,
            feed_id: Uuid,
        ) -> Result<Change, AppError> {
            self.0
                .themes
                .lock()
                .insert(theme.id().to_string(), (theme.clone(), feed_id));
            Ok(self.0.record(Entity::Theme, feed_id, ChangeOp::Insert))
        }

        async fn update(
            &mut self,
            theme: &InstalledTheme,
            previous: Version,
        ) -> Result<Change, AppError> {
            let feed_id = {
                let mut themes = self.0.themes.lock();
                let entry = themes.get_mut(theme.id().as_str()).unwrap();
                if entry.0.record().version != previous {
                    return Err(AppError::version_conflict("theme", theme.id()));
                }
                entry.0 = theme.clone();
                entry.1
            };
            Ok(self.0.record(Entity::Theme, feed_id, ChangeOp::Update))
        }

        async fn delete(
            &mut self,
            id: &ThemeId,
            expected: Version,
            _at: Timestamp,
        ) -> Result<Option<Change>, AppError> {
            let found = self.0.themes.lock().get(id.as_str()).cloned();
            let Some((theme, feed_id)) = found else {
                return Ok(None);
            };
            if theme.record().version != expected {
                return Err(AppError::version_conflict("theme", id));
            }
            self.0.themes.lock().remove(id.as_str());
            Ok(Some(self.0.record(
                Entity::Theme,
                feed_id,
                ChangeOp::Delete,
            )))
        }

        async fn selection(&mut self) -> Result<Option<String>, AppError> {
            Ok(self.0.selection.lock().clone())
        }

        async fn select(&mut self, id: &str, _at: Timestamp) -> Result<Option<Change>, AppError> {
            let mut selection = self.0.selection.lock();
            if selection.as_deref() == Some(id) {
                return Ok(None);
            }
            *selection = Some(id.to_owned());
            drop(selection);
            Ok(Some(self.0.record(
                Entity::Setting,
                SELECTION_FEED_ID,
                ChangeOp::Update,
            )))
        }

        async fn settings(&mut self, id: &ThemeId) -> Result<Option<StoredSettings>, AppError> {
            Ok(self.0.settings.lock().get(id.as_str()).cloned())
        }

        async fn put_settings(
            &mut self,
            id: &ThemeId,
            values: &ThemeSettingValues,
            expected: Option<Version>,
            _at: Timestamp,
        ) -> Result<(Version, Change), AppError> {
            let feed_id = self.0.themes.lock().get(id.as_str()).map(|entry| entry.1);
            let feed_id = feed_id.ok_or_else(|| theme_not_found(id.as_str()))?;
            let mut settings = self.0.settings.lock();
            let current = settings.get(id.as_str()).map(|stored| stored.version);
            if current != expected {
                return Err(AppError::version_conflict("theme settings", id));
            }
            let version = current.map_or(Version::INITIAL, Version::next);
            settings.insert(
                id.to_string(),
                StoredSettings {
                    values: values.clone(),
                    version,
                },
            );
            drop(settings);
            Ok((
                version,
                self.0.record(Entity::Setting, feed_id, ChangeOp::Update),
            ))
        }

        async fn delete_settings(
            &mut self,
            id: &ThemeId,
            expected: Version,
            at: Timestamp,
        ) -> Result<Option<Change>, AppError> {
            let current = self.0.settings.lock().get(id.as_str()).map(|s| s.version);
            match current {
                Some(version) if version != expected => {
                    Err(AppError::version_conflict("theme settings", id))
                }
                Some(_) => self.purge_settings(id, at).await,
                None => Ok(None),
            }
        }

        async fn purge_settings(
            &mut self,
            id: &ThemeId,
            _at: Timestamp,
        ) -> Result<Option<Change>, AppError> {
            let feed_id = self.0.themes.lock().get(id.as_str()).map(|entry| entry.1);
            let removed = self.0.settings.lock().remove(id.as_str());
            Ok(feed_id
                .filter(|_| removed.is_some())
                .map(|feed_id| self.0.record(Entity::Setting, feed_id, ChangeOp::Delete)))
        }

        async fn set_origin(
            &mut self,
            id: &ThemeId,
            source: Option<&SourceId>,
            _at: Timestamp,
        ) -> Result<(), AppError> {
            let mut origins = self.0.origins.lock();
            match source {
                Some(source) => origins.insert(id.to_string(), source.clone()),
                None => origins.remove(id.as_str()),
            };
            Ok(())
        }

        async fn commit(self: Box<Self>) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ThemeRepository for Arc<Memory> {
        async fn begin(&self) -> Result<Box<dyn ThemeWrite>, AppError> {
            Ok(Box::new(Write(self.clone())))
        }

        async fn list(&self) -> Result<Vec<InstalledTheme>, AppError> {
            Ok(self
                .themes
                .lock()
                .values()
                .map(|(theme, _)| theme.clone())
                .collect())
        }

        async fn get(&self, id: &ThemeId) -> Result<Option<InstalledTheme>, AppError> {
            Ok(self
                .themes
                .lock()
                .get(id.as_str())
                .map(|(theme, _)| theme.clone()))
        }

        async fn selection(&self) -> Result<Option<String>, AppError> {
            Ok(self.selection.lock().clone())
        }

        async fn settings(&self, id: &ThemeId) -> Result<Option<StoredSettings>, AppError> {
            Ok(self.settings.lock().get(id.as_str()).cloned())
        }

        async fn origins(&self) -> Result<BTreeMap<ThemeId, SourceId>, AppError> {
            Ok(self
                .origins
                .lock()
                .iter()
                .map(|(id, source)| (ThemeId::parse(id).unwrap(), source.clone()))
                .collect())
        }
    }

    /// Packages are archives of the form `path=text` lines in these tests.
    #[derive(Default)]
    pub(crate) struct MemoryFiles {
        stored: Mutex<HashMap<String, Vec<PackageFile>>>,
        removals: Mutex<Vec<String>>,
        fail_remove: Mutex<bool>,
        /// When set, `store` announces itself and waits to be released.
        hold_store: Mutex<bool>,
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    pub(crate) fn archive(files: &[PackageFile]) -> Vec<u8> {
        let mut text = String::new();
        for file in files {
            let data = String::from_utf8_lossy(&file.data).replace('\n', "\\n");
            text += &[file.path.as_str(), "=", data.as_str(), "\n"].concat();
        }
        text.into_bytes()
    }

    #[async_trait]
    impl ThemeFiles for MemoryFiles {
        async fn unpack(&self, archive: Vec<u8>) -> Result<Vec<PackageFile>, AppError> {
            let text = String::from_utf8(archive)
                .map_err(|_| crate::theme_package::package_invalid("not an archive"))?;
            Ok(text
                .lines()
                .filter_map(|line| line.split_once('='))
                .map(|(path, data)| PackageFile {
                    path: path.to_owned(),
                    data: data.replace("\\n", "\n").into_bytes(),
                })
                .collect())
        }

        async fn store(&self, id: &ThemeId, files: Vec<PackageFile>) -> Result<(), AppError> {
            let hold = *self.hold_store.lock();
            if hold {
                self.entered.notify_one();
                self.release.notified().await;
            }
            self.stored.lock().insert(id.to_string(), files);
            Ok(())
        }

        async fn remove(&self, id: &ThemeId) -> Result<(), AppError> {
            if *self.fail_remove.lock() {
                return Err(AppError::unavailable("files-failed", "disk"));
            }
            self.removals.lock().push(id.to_string());
            self.stored.lock().remove(id.as_str());
            Ok(())
        }

        async fn read(&self, id: &ThemeId, path: &[String]) -> Result<Option<Vec<u8>>, AppError> {
            let joined = path.join("/");
            Ok(self
                .stored
                .lock()
                .get(id.as_str())
                .and_then(|files| files.iter().find(|file| file.path == joined))
                .map(|file| file.data.clone()))
        }

        async fn paths(&self, id: &ThemeId) -> Result<Vec<String>, AppError> {
            let stored = self.stored.lock();
            let files = stored.get(id.as_str()).map_or(&[][..], Vec::as_slice);
            Ok(files.iter().map(|file| file.path.clone()).collect())
        }
    }

    pub(crate) struct Fixture {
        pub(crate) service: ThemeService,
        pub(crate) memory: Arc<Memory>,
        pub(crate) files: Arc<MemoryFiles>,
        pub(crate) signal: ChangeSignal,
    }

    pub(crate) fn fixture() -> Fixture {
        let memory = Arc::new(Memory::default());
        let files = Arc::new(MemoryFiles::default());
        let signal = ChangeSignal::new();
        let service = ThemeService::new(
            Arc::new(memory.clone()),
            files.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        );
        Fixture {
            service,
            memory,
            files,
            signal,
        }
    }

    fn night_blue() -> Vec<u8> {
        archive(&package())
    }

    fn with_settings() -> Vec<u8> {
        let schema = r#""network": [] },
            "settingsSchema": {
                "dim": { "type": "boolean", "label": "Dim", "default": true },
                "mode": { "type": "select", "label": "Mode", "default": "a",
                          "options": [{ "value": "a", "label": "A" }, { "value": "b", "label": "B" }] }
            }"#;
        let mut files = package();
        files[0] = file(
            "manifest.json",
            &MANIFEST.replace(r#""network": [] }"#, schema),
        );
        archive(&files)
    }

    fn input(items: &[(&str, SettingValue)]) -> BTreeMap<String, SettingValue> {
        items
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    fn choice(text: &str) -> SettingValue {
        SettingValue::Choice(text.to_owned())
    }

    #[tokio::test]
    async fn settings_start_empty_and_saving_counts_the_version_up() {
        let fixture = fixture();
        fixture
            .service
            .install(with_settings(), false)
            .await
            .unwrap();
        let none = fixture.service.settings("night-blue").await.unwrap();
        assert_eq!((none.values.is_empty(), none.version), (true, None));
        let first = input(&[("dim", SettingValue::Switch(false))]);
        let saved = fixture
            .service
            .save_settings("night-blue", first, None)
            .await
            .unwrap();
        assert_eq!(saved.version, Some(Version::INITIAL));
        let second = input(&[("mode", choice("b"))]);
        let saved = fixture
            .service
            .save_settings("night-blue", second.clone(), Some(Version::INITIAL))
            .await
            .unwrap();
        assert_eq!(saved.version, Some(Version::from_stored(2)));
        let read = fixture.service.settings("night-blue").await.unwrap();
        assert_eq!(read, saved);
        assert_eq!(read.values.values(), &second);
        let feed = fixture.memory.feed.lock().clone();
        assert_eq!(feed[1].0, Entity::Setting);
        assert_eq!(feed[1].1, feed[0].1);
    }

    fn with_schema(schema: &str) -> Vec<u8> {
        let tail = format!(r#""network": [] }}, "settingsSchema": {schema}"#);
        let mut files = package();
        files[0] = file(
            "manifest.json",
            &MANIFEST.replace(r#""network": [] }"#, &tail),
        );
        archive(&files)
    }

    #[tokio::test]
    async fn values_that_no_longer_fit_a_reinstalled_manifest_are_left_out() {
        let fixture = fixture();
        fixture
            .service
            .install(with_settings(), false)
            .await
            .unwrap();
        let both = input(&[("dim", SettingValue::Switch(false)), ("mode", choice("b"))]);
        fixture
            .service
            .save_settings("night-blue", both, None)
            .await
            .unwrap();
        let reduced = r#"{ "mode": { "type": "select", "label": "Mode", "default": "a",
            "options": [{ "value": "a", "label": "A" }] } }"#;
        fixture
            .service
            .install(with_schema(reduced), false)
            .await
            .unwrap();
        let read = fixture.service.settings("night-blue").await.unwrap();
        assert!(read.values.is_empty());
        let saved = fixture
            .service
            .save_settings("night-blue", input(&[("mode", choice("a"))]), read.version)
            .await
            .unwrap();
        assert_eq!(saved.values.values(), &input(&[("mode", choice("a"))]));
    }

    #[tokio::test]
    async fn a_removed_key_does_not_come_back_from_the_store() {
        let fixture = fixture();
        fixture
            .service
            .install(with_settings(), false)
            .await
            .unwrap();
        let both = input(&[("dim", SettingValue::Switch(false)), ("mode", choice("b"))]);
        fixture
            .service
            .save_settings("night-blue", both, None)
            .await
            .unwrap();
        let kept = r#"{ "mode": { "type": "select", "label": "Mode", "default": "a",
            "options": [{ "value": "a", "label": "A" }, { "value": "b", "label": "B" }] } }"#;
        fixture
            .service
            .install(with_schema(kept), false)
            .await
            .unwrap();
        let read = fixture.service.settings("night-blue").await.unwrap();
        assert_eq!(read.values.values(), &input(&[("mode", choice("b"))]));
        fixture
            .service
            .save_settings("night-blue", read.values.values().clone(), read.version)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn saving_refuses_stale_versions_and_bad_values_without_a_change() {
        let fixture = fixture();
        fixture
            .service
            .install(with_settings(), false)
            .await
            .unwrap();
        let on = input(&[("dim", SettingValue::Switch(true))]);
        fixture
            .service
            .save_settings("night-blue", on.clone(), None)
            .await
            .unwrap();
        for expected in [None, Some(Version::from_stored(5))] {
            let error = fixture
                .service
                .save_settings("night-blue", on.clone(), expected)
                .await
                .unwrap_err();
            assert_eq!(error.code(), "version-conflict");
        }
        let bad = input(&[("mode", choice("z")), ("other", choice("a"))]);
        let error = fixture
            .service
            .save_settings("night-blue", bad, Some(Version::INITIAL))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-settings-invalid");
        let fields: Vec<_> = error.fields().iter().map(|f| f.field.as_str()).collect();
        assert_eq!(fields, ["mode", "other"]);
        assert_eq!(fixture.memory.feed.lock().len(), 2);
    }

    #[tokio::test]
    async fn settings_of_an_unknown_theme_are_not_found() {
        let fixture = fixture();
        let on = input(&[("dim", SettingValue::Switch(true))]);
        let error = fixture
            .service
            .save_settings("night-blue", on, None)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-not-found");
        let error = fixture.service.settings("night-blue").await.unwrap_err();
        assert_eq!(error.code(), "theme-not-found");
        let error = fixture.service.settings("Bad Id").await.unwrap_err();
        assert_eq!(error.code(), "theme-id-invalid");
        let error = fixture
            .service
            .reset_settings("night-blue", None)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-not-found");
    }

    #[tokio::test]
    async fn a_theme_without_settings_refuses_every_key() {
        let fixture = fixture();
        fixture.service.install(night_blue(), false).await.unwrap();
        let on = input(&[("dim", SettingValue::Switch(true))]);
        let error = fixture
            .service
            .save_settings("night-blue", on, None)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-settings-invalid");
        let empty = fixture
            .service
            .save_settings("night-blue", BTreeMap::new(), None)
            .await
            .unwrap();
        assert!(empty.values.is_empty());
    }

    #[tokio::test]
    async fn resetting_checks_the_version_and_is_idempotent() {
        let fixture = fixture();
        fixture
            .service
            .install(with_settings(), false)
            .await
            .unwrap();
        fixture
            .service
            .reset_settings("night-blue", None)
            .await
            .unwrap();
        assert_eq!(fixture.memory.feed.lock().len(), 1);
        let on = input(&[("dim", SettingValue::Switch(false))]);
        fixture
            .service
            .save_settings("night-blue", on, None)
            .await
            .unwrap();
        for expected in [None, Some(Version::from_stored(3))] {
            let error = fixture
                .service
                .reset_settings("night-blue", expected)
                .await
                .unwrap_err();
            assert_eq!(error.code(), "version-conflict");
        }
        fixture
            .service
            .reset_settings("night-blue", Some(Version::INITIAL))
            .await
            .unwrap();
        let read = fixture.service.settings("night-blue").await.unwrap();
        assert_eq!((read.values.is_empty(), read.version), (true, None));
        assert_eq!(
            fixture.memory.feed.lock().last().unwrap().2,
            ChangeOp::Delete
        );
    }

    #[tokio::test]
    async fn removing_a_theme_removes_its_values_and_a_reinstall_starts_clean() {
        let fixture = fixture();
        let theme = fixture
            .service
            .install(with_settings(), false)
            .await
            .unwrap()
            .theme;
        let on = input(&[("dim", SettingValue::Switch(false))]);
        fixture
            .service
            .save_settings("night-blue", on, None)
            .await
            .unwrap();
        fixture
            .service
            .remove("night-blue", theme.record().version)
            .await
            .unwrap();
        assert!(fixture.memory.settings.lock().is_empty());
        fixture
            .service
            .install(with_settings(), false)
            .await
            .unwrap();
        let read = fixture.service.settings("night-blue").await.unwrap();
        assert_eq!(read.version, None);
    }

    #[tokio::test]
    async fn a_removal_waits_for_an_install_in_progress() {
        let fixture = fixture();
        fixture.service.install(night_blue(), false).await.unwrap();
        *fixture.files.hold_store.lock() = true;
        let mut files = package();
        files[0] = file("manifest.json", &MANIFEST.replace("1.0.0", "1.1.0"));
        let files_gate = &fixture.files;
        let waited = async {
            files_gate.entered.notified().await;
            let removal = fixture.service.remove("night-blue", Version::INITIAL);
            let attempt = tokio::time::timeout(std::time::Duration::from_millis(50), removal).await;
            files_gate.release.notify_one();
            attempt.is_err()
        };
        let (installed, waited) =
            tokio::join!(fixture.service.install(archive(&files), false), waited);
        installed.unwrap();
        assert!(
            waited,
            "the removal ran while the install was storing files"
        );
        assert_eq!(fixture.service.list().await.unwrap().themes.len(), 1);
    }

    #[tokio::test]
    async fn files_of_a_theme_that_is_not_installed_are_not_served() {
        let fixture = fixture();
        fixture.service.install(night_blue(), false).await.unwrap();
        fixture.memory.themes.lock().clear();
        let missing = fixture.service.asset("night-blue", "theme.css").await;
        assert!(missing.is_err());
    }

    #[tokio::test]
    async fn the_default_is_active_before_anything_is_chosen() {
        let fixture = fixture();
        let list = fixture.service.list().await.unwrap();
        assert_eq!(list.themes.len(), 0);
        assert_eq!(list.active, "nullradix");
    }

    #[tokio::test]
    async fn installing_stores_the_files_then_lists_the_theme() {
        let fixture = fixture();
        let theme = fixture
            .service
            .install(night_blue(), false)
            .await
            .unwrap()
            .theme;
        assert_eq!(theme.id().as_str(), "night-blue");
        assert_eq!(theme.record().version, Version::INITIAL);
        assert_eq!(fixture.files.stored.lock().len(), 1);
        let list = fixture.service.list().await.unwrap();
        assert_eq!(list.themes.len(), 1);
        let feed = fixture.memory.feed.lock().clone();
        assert_eq!(feed.len(), 1);
        assert_eq!((feed[0].0, feed[0].2), (Entity::Theme, ChangeOp::Insert));
    }

    #[tokio::test]
    async fn installing_the_same_id_again_replaces_it_and_bumps_the_version() {
        let fixture = fixture();
        fixture.service.install(night_blue(), false).await.unwrap();
        let mut files = package();
        files[0] = file("manifest.json", &MANIFEST.replace("1.0.0", "1.1.0"));
        let again = fixture
            .service
            .install(archive(&files), false)
            .await
            .unwrap()
            .theme;
        assert_eq!(again.record().version, Version::from_stored(2));
        assert_eq!(again.record().manifest.record().version, "1.1.0");
        assert_eq!(fixture.service.list().await.unwrap().themes.len(), 1);
        assert_eq!(fixture.memory.feed.lock()[1].2, ChangeOp::Update);
    }

    #[tokio::test]
    async fn an_invalid_package_stores_nothing() {
        let fixture = fixture();
        let mut files = package();
        files.push(file("../evil.css", "x"));
        let error = fixture
            .service
            .install(archive(&files), false)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-package-invalid");
        let broken = vec![file("manifest.json", "{"), file("theme.css", "a {}")];
        let error = fixture
            .service
            .install(archive(&broken), false)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-manifest-invalid");
        assert!(fixture.files.stored.lock().is_empty());
        assert_eq!(fixture.service.list().await.unwrap().themes.len(), 0);
    }

    #[tokio::test]
    async fn a_package_may_not_take_the_name_of_a_built_in_theme() {
        let fixture = fixture();
        let files = vec![
            file("manifest.json", &MANIFEST.replace("night-blue", "light")),
            file("theme.css", "a {}"),
        ];
        let error = fixture
            .service
            .install(archive(&files), false)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-id-reserved");
        assert!(fixture.files.stored.lock().is_empty());
    }

    #[tokio::test]
    async fn only_built_in_or_installed_themes_can_be_chosen() {
        let fixture = fixture();
        assert_eq!(fixture.service.select("dark").await.unwrap(), "dark");
        assert_eq!(fixture.service.list().await.unwrap().active, "dark");
        let error = fixture.service.select("night-blue").await.unwrap_err();
        assert_eq!(error.code(), "theme-not-found");
        let error = fixture.service.select("Bad Id").await.unwrap_err();
        assert_eq!(error.code(), "theme-id-invalid");
        fixture.service.install(night_blue(), false).await.unwrap();
        fixture.service.select("night-blue").await.unwrap();
        assert_eq!(fixture.service.list().await.unwrap().active, "night-blue");
    }

    #[tokio::test]
    async fn choosing_the_same_theme_again_is_no_change() {
        let fixture = fixture();
        let mut head = fixture.signal.subscribe();
        fixture.service.select("light").await.unwrap();
        assert!(head.has_changed().unwrap());
        head.borrow_and_update();
        fixture.service.select("light").await.unwrap();
        assert!(!head.has_changed().unwrap());
    }

    #[tokio::test]
    async fn removing_the_active_theme_falls_back_to_the_default() {
        let fixture = fixture();
        let theme = fixture
            .service
            .install(night_blue(), false)
            .await
            .unwrap()
            .theme;
        fixture.service.select("night-blue").await.unwrap();
        fixture
            .service
            .remove("night-blue", theme.record().version)
            .await
            .unwrap();
        let list = fixture.service.list().await.unwrap();
        assert_eq!(list.themes.len(), 0);
        assert_eq!(list.active, "nullradix");
        assert_eq!(fixture.files.removals.lock().as_slice(), ["night-blue"]);
        let kinds: Vec<_> = fixture
            .memory
            .feed
            .lock()
            .iter()
            .map(|entry| entry.0)
            .collect();
        assert_eq!(
            kinds,
            [
                Entity::Theme,
                Entity::Setting,
                Entity::Theme,
                Entity::Setting
            ]
        );
    }

    #[tokio::test]
    async fn removing_another_theme_keeps_the_choice() {
        let fixture = fixture();
        let theme = fixture
            .service
            .install(night_blue(), false)
            .await
            .unwrap()
            .theme;
        fixture.service.select("dark").await.unwrap();
        fixture
            .service
            .remove("night-blue", theme.record().version)
            .await
            .unwrap();
        assert_eq!(fixture.service.list().await.unwrap().active, "dark");
    }

    #[tokio::test]
    async fn removing_checks_version_and_presence() {
        let fixture = fixture();
        let error = fixture
            .service
            .remove("night-blue", Version::INITIAL)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-not-found");
        fixture.service.install(night_blue(), false).await.unwrap();
        let error = fixture
            .service
            .remove("night-blue", Version::from_stored(7))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "version-conflict");
        assert_eq!(fixture.service.list().await.unwrap().themes.len(), 1);
        let error = fixture
            .service
            .remove("../x", Version::INITIAL)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-id-invalid");
    }

    #[tokio::test]
    async fn failing_file_removal_does_not_undo_the_uninstall() {
        let fixture = fixture();
        let theme = fixture
            .service
            .install(night_blue(), false)
            .await
            .unwrap()
            .theme;
        *fixture.files.fail_remove.lock() = true;
        fixture
            .service
            .remove("night-blue", theme.record().version)
            .await
            .unwrap();
        assert_eq!(fixture.service.list().await.unwrap().themes.len(), 0);
    }

    #[tokio::test]
    async fn a_stored_choice_without_its_package_shows_as_the_default() {
        let fixture = fixture();
        *fixture.memory.selection.lock() = Some(String::from("gone"));
        assert_eq!(fixture.service.list().await.unwrap().active, "nullradix");
    }

    #[tokio::test]
    async fn assets_are_served_with_their_type_and_unsafe_paths_are_not_found() {
        let fixture = fixture();
        fixture.service.install(night_blue(), false).await.unwrap();
        let css = fixture
            .service
            .asset("night-blue", "theme.css")
            .await
            .unwrap();
        assert_eq!(css.content_type, "text/css; charset=utf-8");
        assert_eq!(css.data, b":root { --accent: #6c63ff }");
        for (id, path) in [
            ("night-blue", "missing.css"),
            ("night-blue", "../night-blue/theme.css"),
            ("night-blue", "a/../theme.css"),
            ("night-blue", "/theme.css"),
            ("night-blue", "theme.css/"),
            ("night-blue", ""),
            ("night-blue", "a\\b"),
            ("Night", "theme.css"),
            ("..", "theme.css"),
            ("other-theme", "theme.css"),
        ] {
            let error = fixture.service.asset(id, path).await.unwrap_err();
            assert_eq!(error.code(), "theme-file-not-found", "{id} {path}");
        }
    }

    fn risky(version: &str, network: &str, code: bool) -> Vec<u8> {
        let manifest = MANIFEST
            .replace("1.0.0", version)
            .replace(r#""network": []"#, &format!(r#""network": [{network}]"#));
        let mut files = vec![
            file("manifest.json", &manifest),
            file("theme.css", ":root { --accent: #6c63ff }"),
        ];
        if code {
            files.push(file("main.js", "export default () => {}"));
        }
        archive(&files)
    }

    async fn refused(fixture: &Fixture, bytes: Vec<u8>) -> AppError {
        let error = fixture.service.install(bytes, false).await.unwrap_err();
        assert_eq!(error.code(), "theme-confirmation-required");
        assert_eq!(error.kind(), crate::ErrorKind::Conflict);
        error
    }

    #[tokio::test]
    async fn a_new_theme_with_code_or_domains_needs_a_confirmation() {
        let fixture = fixture();
        let error = refused(&fixture, risky("1.0.0", r#""a.example.com""#, true)).await;
        let facts = error.confirmation().unwrap();
        assert!(facts.risk.has_code && !facts.update && facts.added.code);
        assert_eq!(facts.risk.network, ["a.example.com"]);
        assert!(fixture.files.stored.lock().is_empty());
        assert_eq!(fixture.service.list().await.unwrap().themes.len(), 0);
        refused(&fixture, risky("1.0.0", r#""a.example.com""#, false)).await;
        refused(&fixture, risky("1.0.0", "", true)).await;
        let installed = fixture
            .service
            .install(risky("1.0.0", r#""a.example.com""#, true), true)
            .await
            .unwrap();
        assert!(installed.added.code);
        assert_eq!(installed.added.domains, ["a.example.com"]);
        assert_eq!(fixture.files.stored.lock().len(), 1);
    }

    #[tokio::test]
    async fn every_install_of_a_risky_package_needs_a_confirmation_but_a_plain_one_does_not() {
        let fixture = fixture();
        let first = risky("1.0.0", r#""a.example.com""#, true);
        fixture.service.install(first.clone(), true).await.unwrap();
        for bytes in [
            first,
            risky("1.1.0", r#""A.example.com""#, true),
            risky("0.9.0", r#""a.example.com""#, true),
        ] {
            let error = refused(&fixture, bytes.clone()).await;
            let facts = error.confirmation().unwrap();
            assert!(facts.update && facts.risk.has_code && facts.added.is_empty());
            let done = fixture.service.install(bytes, true).await.unwrap();
            assert!(done.added.is_empty());
        }
        let plain = fixture.service.install(night_blue(), false).await.unwrap();
        assert!(plain.added.is_empty());
    }

    #[tokio::test]
    async fn a_claude_part_and_its_scripts_need_a_confirmation() {
        let fixture = fixture();
        let claude = MANIFEST.replace(r#""slots": []"#, r#""slots": [], "claudeTheme": true"#);
        let build = |extra: &[PackageFile]| {
            let mut files = vec![
                file("manifest.json", &claude),
                file("theme.css", "a { color: red }"),
                file("claude/hooks/hooks.json", "{}"),
            ];
            files.extend(extra.iter().cloned());
            archive(&files)
        };
        let error = refused(&fixture, build(&[])).await;
        let facts = error.confirmation().unwrap();
        assert!(facts.risk.claude_theme && !facts.risk.has_code && facts.added.claude_theme);
        let script = [file("claude/hooks/run.ts", "run()")];
        let error = refused(&fixture, build(&script)).await;
        assert!(error.confirmation().unwrap().risk.has_code);
    }

    #[tokio::test]
    async fn an_update_that_adds_code_or_a_domain_needs_a_confirmation() {
        let fixture = fixture();
        fixture.service.install(night_blue(), false).await.unwrap();
        let error = refused(&fixture, risky("1.1.0", "", true)).await;
        let facts = error.confirmation().unwrap();
        assert!(facts.update && facts.added.code && facts.added.domains.is_empty());
        let error = refused(&fixture, risky("1.1.0", r#""b.example.com""#, false)).await;
        let facts = error.confirmation().unwrap();
        assert!(!facts.added.code);
        assert_eq!(facts.added.domains, ["b.example.com"]);
        assert_eq!(
            fixture.service.list().await.unwrap().themes[0]
                .record()
                .version,
            Version::INITIAL
        );
        let updated = fixture
            .service
            .install(risky("1.1.0", r#""b.example.com""#, true), true)
            .await
            .unwrap();
        assert!(updated.added.code);
        let more = risky("1.2.0", r#""b.example.com", "c.example.com""#, true);
        let error = refused(&fixture, more).await;
        assert_eq!(
            error.confirmation().unwrap().added.domains,
            ["c.example.com"]
        );
    }
}
