//! Themes: installed packages, the chosen theme and the files a theme serves.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    BUILTIN_THEMES, CODE_FILE, DEFAULT_THEME, InstalledTheme, RiskAdded, SettingValue, SourceId,
    ThemeId, ThemeManifest, ThemeRisk, ThemeSettingValues, Timestamp, Version, asset_segments,
    content_type, inspect_package,
};
use std::collections::{BTreeMap, BTreeSet};
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

/// The code a theme brings for the web UI and whether the person allowed it to run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThemeCode {
    /// The package has a `main.js`.
    pub has_code: bool,
    /// The code switch is on; always off without a `main.js`.
    pub enabled: bool,
}

/// A theme that was installed and what its package added to the version it replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub theme: InstalledTheme,
    /// Code, domains and Claude part added to the replaced version.
    pub added: RiskAdded,
    pub code: ThemeCode,
}

/// "a", "a and b" or "a, b and c".
fn join_parts(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
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
        format!("{} contains {}", manifest.id(), join_parts(&parts)),
    )
    .with_confirmation(ConfirmationFacts {
        risk,
        added,
        update,
        id: manifest.id().to_string(),
        name: manifest.record().name.clone(),
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
    /// Whether the code switch of the theme is on; off until someone turned it on.
    async fn code_enabled(&mut self, id: &ThemeId) -> Result<bool, AppError>;
    /// Turns the code switch on or off; `None` when it already is as asked.
    async fn set_code(
        &mut self,
        id: &ThemeId,
        enabled: bool,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
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
    /// The themes whose code switch is on.
    async fn code_switches(&self) -> Result<BTreeSet<ThemeId>, AppError>;
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
    /// The paths of the files of the theme, empty without a folder; `None` when they are unknown.
    async fn paths(&self, id: &ThemeId) -> Option<Vec<String>>;
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

    async fn paths(&self, _id: &ThemeId) -> Option<Vec<String>> {
        Some(Vec::new())
    }
}

fn unavailable() -> AppError {
    AppError::unavailable("theme-files-failed", "this host keeps no theme files")
}

/// A token tried to turn theme code on; only the person in the app may.
#[must_use]
pub fn code_needs_person() -> AppError {
    AppError::forbidden(
        "theme-code-needs-person",
        "only the person in the app can turn theme code on; tokens may only turn it off",
    )
}

fn is_script(path: &str) -> bool {
    content_type(path) == content_type(CODE_FILE)
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

    /// Installs or replaces the theme; code, network domains or a Claude part need `confirmed`.
    pub async fn install(
        &self,
        archive: Vec<u8>,
        confirmed: bool,
        by_person: bool,
    ) -> Result<Installed, AppError> {
        self.install_expecting(archive, None, None, confirmed, by_person)
            .await
    }

    /// Installs the expected package; only the person's install can keep the code switch on.
    pub async fn install_expecting(
        &self,
        archive: Vec<u8>,
        expected: Option<(&ThemeId, &str)>,
        origin: Option<Origin<'_>>,
        confirmed: bool,
        by_person: bool,
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
        let (risk, added) = self.inspect(&manifest, &files, known.as_ref()).await;
        if !confirmed && risk.is_risky() {
            return Err(confirmation_required(
                &manifest,
                risk,
                added,
                known.is_some(),
            ));
        }
        let has_code = files.iter().any(|file| file.path == CODE_FILE);
        let now = self.clock.now();
        // what the person allowed is not what this package brings; off before its files appear
        if known.is_none() || !added.is_empty() || !has_code || !by_person {
            let mut off = self.repository.begin().await?;
            let change = off.set_code(manifest.id(), false, now).await?;
            off.commit().await?;
            if let Some(change) = &change {
                self.signal.notify(change);
            }
        }
        self.files.store(manifest.id(), files).await?;
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
        let enabled = write.code_enabled(theme.id()).await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(Installed {
            theme,
            added,
            code: ThemeCode { has_code, enabled },
        })
    }

    /// The risk of the package and what it brings that the installed version did not have.
    async fn inspect(
        &self,
        manifest: &ThemeManifest,
        files: &[PackageFile],
        known: Option<&InstalledTheme>,
    ) -> (ThemeRisk, RiskAdded) {
        let risk = inspect_package(
            manifest.record(),
            files.iter().map(|file| file.path.as_str()),
        );
        let old = match known {
            Some(known) => {
                // unknown files only leave the code unknown; the stored manifest has the rest
                let paths = self.files.paths(known.id()).await.unwrap_or_default();
                Some(inspect_package(
                    known.record().manifest.record(),
                    paths.iter().map(String::as_str),
                ))
            }
            None => None,
        };
        let added = risk.added_to(old.as_ref());
        (risk, added)
    }

    /// The catalog source of every theme installed from one.
    pub async fn origins(&self) -> Result<BTreeMap<ThemeId, SourceId>, AppError> {
        self.repository.origins().await
    }

    /// Whether the folder of the theme has a `main.js`; unknown files count as having one.
    async fn has_code(&self, id: &ThemeId) -> bool {
        self.files
            .paths(id)
            .await
            .is_none_or(|paths| paths.iter().any(|path| path == CODE_FILE))
    }

    /// The code state of each theme: what it brings and what the person allowed.
    pub async fn codes(
        &self,
        themes: &[InstalledTheme],
    ) -> Result<BTreeMap<ThemeId, ThemeCode>, AppError> {
        let switched = self.repository.code_switches().await?;
        let mut codes = BTreeMap::new();
        for theme in themes {
            let has_code = self.has_code(theme.id()).await;
            let enabled = has_code && switched.contains(theme.id());
            codes.insert(theme.id().clone(), ThemeCode { has_code, enabled });
        }
        Ok(codes)
    }

    /// Turns the code of the theme on for the version the person saw, or off for any version.
    pub async fn set_code(
        &self,
        id: &str,
        enabled: bool,
        expected: Option<Version>,
    ) -> Result<(InstalledTheme, ThemeCode), AppError> {
        let theme_id = ThemeId::parse(id).map_err(|error| id_invalid(&error))?;
        // installs and removals wait, so the files and the row stay the ones checked here
        let _writing = self.writes.lock().await;
        let theme = self
            .repository
            .get(&theme_id)
            .await?
            .ok_or_else(|| theme_not_found(id))?;
        if enabled {
            let Some(expected) = expected else {
                return Err(AppError::invalid(
                    "theme-version-required",
                    "turning the code on needs the version of the theme the person saw",
                ));
            };
            if theme.record().version != expected {
                return Err(AppError::version_conflict("theme", id));
            }
        }
        let has_code = self.has_code(&theme_id).await;
        if enabled && !has_code {
            return Err(AppError::invalid(
                "theme-has-no-code",
                format!("{id} has no {CODE_FILE} to run"),
            ));
        }
        let mut write = self.repository.begin().await?;
        let change = write.set_code(&theme_id, enabled, self.clock.now()).await?;
        write.commit().await?;
        if let Some(change) = &change {
            self.signal.notify(change);
        }
        Ok((theme, ThemeCode { has_code, enabled }))
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
        // the values and the code switch go with the theme, in the same transaction
        let purged = write.purge_settings(&theme, now).await?;
        let switched = write.set_code(&theme, false, now).await?;
        write.set_origin(&theme, None, now).await?;
        let Some(change) = write.delete(&theme, expected, now).await? else {
            return Err(theme_not_found(id));
        };
        let mut changes = vec![change];
        changes.extend(purged);
        changes.extend(switched);
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

    async fn code_is_on(&self, id: &ThemeId) -> Result<bool, AppError> {
        Ok(self.repository.code_switches().await?.contains(id))
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
        // script files are served only while the person's code switch is on
        let script = is_script(path);
        if script && !self.code_is_on(&theme).await? {
            return Err(file_not_found());
        }
        let data = self
            .files
            .read(&theme, &segments)
            .await?
            .ok_or_else(file_not_found)?;
        // an install may have turned the switch off while the file was read
        if script && !self.code_is_on(&theme).await? {
            return Err(file_not_found());
        }
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
        code: Mutex<BTreeSet<String>>,
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

        async fn code_enabled(&mut self, id: &ThemeId) -> Result<bool, AppError> {
            Ok(self.0.code.lock().contains(id.as_str()))
        }

        async fn set_code(
            &mut self,
            id: &ThemeId,
            enabled: bool,
            _at: Timestamp,
        ) -> Result<Option<Change>, AppError> {
            let feed_id = self.0.themes.lock().get(id.as_str()).map(|entry| entry.1);
            let changed = if enabled {
                self.0.code.lock().insert(id.to_string())
            } else {
                self.0.code.lock().remove(id.as_str())
            };
            Ok(feed_id
                .filter(|_| changed)
                .map(|feed_id| self.0.record(Entity::Setting, feed_id, ChangeOp::Update)))
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

        async fn code_switches(&self) -> Result<BTreeSet<ThemeId>, AppError> {
            Ok(self
                .code
                .lock()
                .iter()
                .map(|id| ThemeId::parse(id).unwrap())
                .collect())
        }
    }

    /// Packages are archives of the form `path=text` lines in these tests.
    #[derive(Default)]
    pub(crate) struct MemoryFiles {
        stored: Mutex<HashMap<String, Vec<PackageFile>>>,
        removals: Mutex<Vec<String>>,
        fail_remove: Mutex<bool>,
        /// When set, the list of files is unknown.
        unknown_paths: Mutex<bool>,
        /// When set, `store` announces itself and waits to be released.
        hold_store: Mutex<bool>,
        /// When set, `read` announces itself and waits to be released.
        hold_read: Mutex<bool>,
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
            let hold = *self.hold_read.lock();
            if hold {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(self
                .stored
                .lock()
                .get(id.as_str())
                .and_then(|files| files.iter().find(|file| file.path == joined))
                .map(|file| file.data.clone()))
        }

        async fn paths(&self, id: &ThemeId) -> Option<Vec<String>> {
            if *self.unknown_paths.lock() {
                return None;
            }
            let stored = self.stored.lock();
            let files = stored.get(id.as_str()).map_or(&[][..], Vec::as_slice);
            Some(files.iter().map(|file| file.path.clone()).collect())
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
            .install(with_settings(), false, true)
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
            .install(with_settings(), false, true)
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
            .install(with_schema(reduced), false, true)
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
            .install(with_settings(), false, true)
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
            .install(with_schema(kept), false, true)
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
            .install(with_settings(), false, true)
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
        fixture
            .service
            .install(night_blue(), false, true)
            .await
            .unwrap();
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
            .install(with_settings(), false, true)
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
            .install(with_settings(), false, true)
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
            .install(with_settings(), false, true)
            .await
            .unwrap();
        let read = fixture.service.settings("night-blue").await.unwrap();
        assert_eq!(read.version, None);
    }

    #[tokio::test]
    async fn a_removal_waits_for_an_install_in_progress() {
        let fixture = fixture();
        fixture
            .service
            .install(night_blue(), false, true)
            .await
            .unwrap();
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
        let (installed, waited) = tokio::join!(
            fixture.service.install(archive(&files), false, true),
            waited
        );
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
        fixture
            .service
            .install(night_blue(), false, true)
            .await
            .unwrap();
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
            .install(night_blue(), false, true)
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
        fixture
            .service
            .install(night_blue(), false, true)
            .await
            .unwrap();
        let mut files = package();
        files[0] = file("manifest.json", &MANIFEST.replace("1.0.0", "1.1.0"));
        let again = fixture
            .service
            .install(archive(&files), false, true)
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
            .install(archive(&files), false, true)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-package-invalid");
        let broken = vec![file("manifest.json", "{"), file("theme.css", "a {}")];
        let error = fixture
            .service
            .install(archive(&broken), false, true)
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
            .install(archive(&files), false, true)
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
        fixture
            .service
            .install(night_blue(), false, true)
            .await
            .unwrap();
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
            .install(night_blue(), false, true)
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
            .install(night_blue(), false, true)
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
        fixture
            .service
            .install(night_blue(), false, true)
            .await
            .unwrap();
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
            .install(night_blue(), false, true)
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
        fixture
            .service
            .install(night_blue(), false, true)
            .await
            .unwrap();
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
        let error = fixture
            .service
            .install(bytes, false, true)
            .await
            .unwrap_err();
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
        assert_eq!(
            (facts.id.as_str(), facts.name.as_str()),
            ("night-blue", "Night Blue")
        );
        assert_eq!(
            error.message(),
            "night-blue contains code and network domains a.example.com"
        );
        assert!(fixture.files.stored.lock().is_empty());
        assert_eq!(fixture.service.list().await.unwrap().themes.len(), 0);
        refused(&fixture, risky("1.0.0", r#""a.example.com""#, false)).await;
        refused(&fixture, risky("1.0.0", "", true)).await;
        let installed = fixture
            .service
            .install(risky("1.0.0", r#""a.example.com""#, true), true, true)
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
        fixture
            .service
            .install(first.clone(), true, true)
            .await
            .unwrap();
        for bytes in [
            first,
            risky("1.1.0", r#""A.example.com""#, true),
            risky("0.9.0", r#""a.example.com""#, true),
        ] {
            let error = refused(&fixture, bytes.clone()).await;
            let facts = error.confirmation().unwrap();
            assert!(facts.update && facts.risk.has_code && facts.added.is_empty());
            let done = fixture.service.install(bytes, true, true).await.unwrap();
            assert!(done.added.is_empty());
        }
        let plain = fixture
            .service
            .install(night_blue(), false, true)
            .await
            .unwrap();
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
        assert_eq!(error.message(), "night-blue contains a Claude part");
        let script = [file("claude/hooks/run.ts", "run()")];
        let error = refused(&fixture, build(&script)).await;
        assert!(error.confirmation().unwrap().risk.has_code);
        assert_eq!(
            error.message(),
            "night-blue contains code and a Claude part"
        );
    }

    #[tokio::test]
    async fn an_update_that_adds_code_or_a_domain_needs_a_confirmation() {
        let fixture = fixture();
        fixture
            .service
            .install(night_blue(), false, true)
            .await
            .unwrap();
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
            .install(risky("1.1.0", r#""b.example.com""#, true), true, true)
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

    /// A package with a Claude part, a domain and the given extra files.
    fn everything(version: &str, extra: &[PackageFile]) -> Vec<u8> {
        let manifest = MANIFEST
            .replace("1.0.0", version)
            .replace(r#""slots": []"#, r#""slots": [], "claudeTheme": true"#)
            .replace(r#""network": []"#, r#""network": ["a.example.com"]"#);
        let mut files = vec![
            file("manifest.json", &manifest),
            file("theme.css", "a { color: red }"),
            file("claude/hooks/hooks.json", "{}"),
        ];
        files.extend(extra.iter().cloned());
        archive(&files)
    }

    #[tokio::test]
    async fn unknown_old_files_leave_only_the_code_unknown() {
        let fixture = fixture();
        let main = [file("main.js", "export default () => {}")];
        fixture
            .service
            .install(everything("1.0.0", &main), true, true)
            .await
            .unwrap();
        *fixture.files.unknown_paths.lock() = true;
        let again = fixture
            .service
            .install(everything("1.0.1", &main), true, true)
            .await
            .unwrap();
        // the stored manifest still knows the domains and the Claude part
        assert!(again.added.code);
        assert_eq!(again.added.domains, Vec::<String>::new());
        assert!(!again.added.claude_theme);
        let error = fixture
            .service
            .install(everything("1.0.2", &main), false, true)
            .await
            .unwrap_err();
        let facts = error.confirmation().unwrap();
        assert!(facts.added.code && !facts.added.claude_theme && facts.added.domains.is_empty());
    }

    fn with_code(version: &str) -> Vec<u8> {
        risky(version, r#""a.example.com""#, true)
    }

    async fn code_of(fixture: &Fixture) -> (Version, ThemeCode) {
        let list = fixture.service.list().await.unwrap();
        let codes = fixture.service.codes(&list.themes).await.unwrap();
        let theme = &list.themes[0];
        (theme.record().version, codes[theme.id()])
    }

    #[tokio::test]
    async fn the_code_switch_is_off_until_it_is_turned_on() {
        let fixture = fixture();
        let installed = fixture
            .service
            .install(with_code("1.0.0"), true, true)
            .await
            .unwrap();
        assert_eq!(
            installed.code,
            ThemeCode {
                has_code: true,
                enabled: false
            }
        );
        let (version, code) = code_of(&fixture).await;
        assert!(code.has_code && !code.enabled);

        let (_, on) = fixture
            .service
            .set_code("night-blue", true, Some(version))
            .await
            .unwrap();
        assert!(on.enabled);
        assert!(code_of(&fixture).await.1.enabled);
        // the theme row itself does not change
        assert_eq!(code_of(&fixture).await.0, version);
        let (_, off) = fixture
            .service
            .set_code("night-blue", false, Some(version))
            .await
            .unwrap();
        assert!(!off.enabled);
        assert!(!code_of(&fixture).await.1.enabled);
    }

    #[tokio::test]
    async fn setting_the_switch_checks_the_theme_and_its_version() {
        let fixture = fixture();
        let missing = fixture
            .service
            .set_code("night-blue", true, Some(Version::INITIAL))
            .await
            .unwrap_err();
        assert_eq!(missing.code(), "theme-not-found");
        let invalid = fixture
            .service
            .set_code("Night", true, Some(Version::INITIAL))
            .await
            .unwrap_err();
        assert_eq!(invalid.code(), "theme-id-invalid");

        fixture
            .service
            .install(night_blue(), false, true)
            .await
            .unwrap();
        let plain = fixture
            .service
            .set_code("night-blue", true, Some(Version::INITIAL))
            .await
            .unwrap_err();
        assert_eq!(plain.code(), "theme-has-no-code");
        assert_eq!(plain.kind(), crate::ErrorKind::Invalid);
        // turning it off needs no code
        fixture
            .service
            .set_code("night-blue", false, None)
            .await
            .unwrap();

        fixture
            .service
            .install(with_code("1.1.0"), true, true)
            .await
            .unwrap();
        let stale = fixture
            .service
            .set_code("night-blue", true, Some(Version::INITIAL))
            .await
            .unwrap_err();
        assert_eq!(stale.code(), "version-conflict");
        assert!(!code_of(&fixture).await.1.enabled);
    }

    #[tokio::test]
    async fn an_update_that_adds_something_turns_the_switch_off() {
        let fixture = fixture();
        fixture
            .service
            .install(with_code("1.0.0"), true, true)
            .await
            .unwrap();
        let version = code_of(&fixture).await.0;
        fixture
            .service
            .set_code("night-blue", true, Some(version))
            .await
            .unwrap();

        // nothing new: the choice of the person stays
        let same = fixture
            .service
            .install(with_code("1.0.1"), true, true)
            .await
            .unwrap();
        assert!(same.added.is_empty() && same.code.enabled);
        assert!(code_of(&fixture).await.1.enabled);

        // a new domain
        let more = risky("1.1.0", r#""a.example.com", "b.example.com""#, true);
        let updated = fixture.service.install(more, true, true).await.unwrap();
        assert_eq!(updated.added.domains, ["b.example.com"]);
        assert!(!updated.code.enabled);
        assert!(!code_of(&fixture).await.1.enabled);
        assert!(fixture.memory.code.lock().is_empty());
    }

    #[tokio::test]
    async fn an_update_with_unknown_old_files_turns_the_switch_off() {
        let fixture = fixture();
        fixture
            .service
            .install(with_code("1.0.0"), true, true)
            .await
            .unwrap();
        let version = code_of(&fixture).await.0;
        fixture
            .service
            .set_code("night-blue", true, Some(version))
            .await
            .unwrap();
        *fixture.files.unknown_paths.lock() = true;
        let done = fixture
            .service
            .install(with_code("1.0.1"), true, true)
            .await
            .unwrap();
        assert!(done.added.code && !done.code.enabled);
        assert!(fixture.memory.code.lock().is_empty());
    }

    #[tokio::test]
    async fn a_package_without_main_js_never_keeps_the_switch_on() {
        let fixture = fixture();
        let lib = |version: &str, with_main: bool| {
            let manifest = MANIFEST.replace("1.0.0", version);
            let mut files = vec![
                file("manifest.json", &manifest),
                file("theme.css", "a { color: red }"),
                file("lib.js", "export const a = 1"),
            ];
            if with_main {
                files.push(file("main.js", "import './lib.js'"));
            }
            archive(&files)
        };
        fixture
            .service
            .install(lib("1.0.0", true), true, true)
            .await
            .unwrap();
        let version = code_of(&fixture).await.0;
        fixture
            .service
            .set_code("night-blue", true, Some(version))
            .await
            .unwrap();
        // the library still counts as code, so nothing is added later
        let dropped = fixture
            .service
            .install(lib("1.1.0", false), true, true)
            .await
            .unwrap();
        assert!(dropped.added.is_empty());
        assert!(!dropped.code.has_code && !dropped.code.enabled);
        let back = fixture
            .service
            .install(lib("1.2.0", true), true, true)
            .await
            .unwrap();
        assert!(back.added.is_empty());
        assert!(back.code.has_code && !back.code.enabled);
    }

    #[tokio::test]
    async fn a_new_install_starts_with_the_switch_off_and_removing_drops_it() {
        let fixture = fixture();
        fixture
            .memory
            .code
            .lock()
            .insert(String::from("night-blue"));
        let fresh = fixture
            .service
            .install(with_code("1.0.0"), true, true)
            .await
            .unwrap();
        assert!(!fresh.code.enabled);
        let version = code_of(&fixture).await.0;
        fixture
            .service
            .set_code("night-blue", true, Some(version))
            .await
            .unwrap();
        assert_eq!(fixture.memory.code.lock().len(), 1);
        fixture.service.remove("night-blue", version).await.unwrap();
        assert!(fixture.memory.code.lock().is_empty());
        let again = fixture
            .service
            .install(with_code("1.0.0"), true, true)
            .await
            .unwrap();
        assert!(!again.code.enabled);
    }

    #[tokio::test]
    async fn the_switch_waits_for_an_install_in_progress() {
        let fixture = Arc::new(fixture());
        fixture
            .service
            .install(with_code("1.0.0"), true, true)
            .await
            .unwrap();
        *fixture.files.hold_store.lock() = true;
        let install = {
            let fixture = fixture.clone();
            tokio::spawn(async move {
                fixture
                    .service
                    .install(with_code("1.1.0"), true, true)
                    .await
            })
        };
        fixture.files.entered.notified().await;
        // the install holds the files of 1.1.0 and the row of 1.0.0
        let switch = {
            let fixture = fixture.clone();
            tokio::spawn(async move {
                fixture
                    .service
                    .set_code("night-blue", true, Some(Version::INITIAL))
                    .await
            })
        };
        tokio::task::yield_now().await;
        assert!(!switch.is_finished());
        fixture.files.release.notify_one();
        install.await.unwrap().unwrap();
        // the version the person saw is gone once the install is done
        let error = switch.await.unwrap().unwrap_err();
        assert_eq!(error.code(), "version-conflict");
    }

    #[tokio::test]
    async fn the_switch_change_reaches_the_change_feed() {
        let fixture = fixture();
        fixture
            .service
            .install(with_code("1.0.0"), true, true)
            .await
            .unwrap();
        let before = fixture.memory.feed.lock().len();
        fixture
            .service
            .set_code("night-blue", true, Some(Version::INITIAL))
            .await
            .unwrap();
        assert_eq!(fixture.memory.feed.lock().len(), before + 1);
        // the same state again is no change
        fixture
            .service
            .set_code("night-blue", true, Some(Version::INITIAL))
            .await
            .unwrap();
        assert_eq!(fixture.memory.feed.lock().len(), before + 1);
    }

    #[tokio::test]
    async fn the_switch_reads_off_while_the_files_of_an_update_are_stored() {
        let fixture = Arc::new(fixture());
        fixture
            .service
            .install(with_code("1.0.0"), true, true)
            .await
            .unwrap();
        fixture
            .service
            .set_code("night-blue", true, Some(Version::INITIAL))
            .await
            .unwrap();
        *fixture.files.hold_store.lock() = true;
        let more = risky("1.1.0", r#""a.example.com", "b.example.com""#, true);
        let install = {
            let fixture = fixture.clone();
            tokio::spawn(async move { fixture.service.install(more, true, true).await })
        };
        fixture.files.entered.notified().await;
        // the new files are not stored yet, and no page may run them as allowed
        assert!(!code_of(&fixture).await.1.enabled);
        assert!(
            fixture
                .service
                .asset("night-blue", "main.js")
                .await
                .is_err()
        );
        fixture.files.release.notify_one();
        install.await.unwrap().unwrap();
        assert!(!code_of(&fixture).await.1.enabled);
    }

    #[tokio::test]
    async fn scripts_are_served_only_while_the_switch_is_on() {
        let fixture = fixture();
        let files = vec![
            file("manifest.json", MANIFEST),
            file("theme.css", "a { color: red }"),
            file("main.js", "run()"),
            file("lib/util.mjs", "export {}"),
        ];
        fixture
            .service
            .install(archive(&files), true, true)
            .await
            .unwrap();
        for path in ["main.js", "lib/util.mjs"] {
            let error = fixture.service.asset("night-blue", path).await.unwrap_err();
            assert_eq!(error.code(), "theme-file-not-found", "{path}");
        }
        assert!(
            fixture
                .service
                .asset("night-blue", "theme.css")
                .await
                .is_ok()
        );
        fixture
            .service
            .set_code("night-blue", true, Some(Version::INITIAL))
            .await
            .unwrap();
        let main = fixture
            .service
            .asset("night-blue", "main.js")
            .await
            .unwrap();
        assert_eq!(main.data, b"run()");
        assert!(
            fixture
                .service
                .asset("night-blue", "lib/util.mjs")
                .await
                .is_ok()
        );
        fixture
            .service
            .set_code("night-blue", false, None)
            .await
            .unwrap();
        assert!(
            fixture
                .service
                .asset("night-blue", "main.js")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn an_install_by_anyone_but_the_person_turns_the_switch_off() {
        let fixture = fixture();
        fixture
            .service
            .install(with_code("1.0.0"), true, true)
            .await
            .unwrap();
        let turn_on = async |fixture: &Fixture| {
            let version = code_of(fixture).await.0;
            fixture
                .service
                .set_code("night-blue", true, Some(version))
                .await
                .unwrap();
        };
        turn_on(&fixture).await;

        // the same domains with new code adds nothing, but only the person may keep it on
        let manifest = MANIFEST
            .replace("1.0.0", "1.0.1")
            .replace(r#""network": []"#, r#""network": ["a.example.com"]"#);
        let swapped = archive(&[
            file("manifest.json", &manifest),
            file("theme.css", ":root { --accent: #6c63ff }"),
            file("main.js", "steal()"),
        ]);
        let token = fixture.service.install(swapped, true, false).await.unwrap();
        assert!(token.added.is_empty() && !token.code.enabled);
        assert!(!code_of(&fixture).await.1.enabled);
        assert!(fixture.memory.code.lock().is_empty());
        assert!(
            fixture
                .service
                .asset("night-blue", "main.js")
                .await
                .is_err()
        );

        // the person's own update that adds nothing keeps the switch
        turn_on(&fixture).await;
        let person = fixture
            .service
            .install(with_code("1.0.2"), true, true)
            .await
            .unwrap();
        assert!(person.added.is_empty() && person.code.enabled);
        assert!(code_of(&fixture).await.1.enabled);
    }

    #[tokio::test]
    async fn a_script_is_refused_when_the_switch_goes_off_while_it_is_read() {
        let fixture = Arc::new(fixture());
        fixture
            .service
            .install(with_code("1.0.0"), true, true)
            .await
            .unwrap();
        fixture
            .service
            .set_code("night-blue", true, Some(Version::INITIAL))
            .await
            .unwrap();
        *fixture.files.hold_read.lock() = true;
        let read = {
            let fixture = fixture.clone();
            tokio::spawn(async move { fixture.service.asset("night-blue", "main.js").await })
        };
        fixture.files.entered.notified().await;
        fixture
            .service
            .set_code("night-blue", false, None)
            .await
            .unwrap();
        fixture.files.release.notify_one();
        let error = read.await.unwrap().unwrap_err();
        assert_eq!(error.code(), "theme-file-not-found");
    }

    #[tokio::test]
    async fn turning_on_needs_a_version_and_turning_off_does_not() {
        let fixture = fixture();
        fixture
            .service
            .install(with_code("1.0.0"), true, true)
            .await
            .unwrap();
        let error = fixture
            .service
            .set_code("night-blue", true, None)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-version-required");
        fixture
            .service
            .set_code("night-blue", true, Some(Version::INITIAL))
            .await
            .unwrap();
        // a stale version does not hinder turning it off
        let (_, off) = fixture
            .service
            .set_code("night-blue", false, Some(Version::INITIAL.next().next()))
            .await
            .unwrap();
        assert!(!off.enabled);
        let missing = fixture
            .service
            .set_code("other-theme", false, None)
            .await
            .unwrap_err();
        assert_eq!(missing.code(), "theme-not-found");
    }
}
