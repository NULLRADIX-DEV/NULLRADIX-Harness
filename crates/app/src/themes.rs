//! Themes: installed packages, the chosen theme and the files a theme serves.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    BUILTIN_THEMES, DEFAULT_THEME, InstalledTheme, ThemeId, Timestamp, Version, asset_segments,
    content_type,
};
use uuid::Uuid;

use crate::theme_package::{PackageFile, check_package};
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

/// Entity id of the chosen theme in the change feed, which only takes UUIDs.
pub const SELECTION_FEED_ID: Uuid = Uuid::from_u128(0x6e75_6c6c_7261_6469_7800_7468_656d_6500);

/// A file of an installed theme with the type it is served as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub data: Vec<u8>,
    pub content_type: &'static str,
}

/// The installed themes and which one is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeList {
    pub themes: Vec<InstalledTheme>,
    /// A built-in theme or the id of an installed one.
    pub active: String,
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

    /// Installs the package, or replaces the installed theme with the same id.
    pub async fn install(&self, archive: Vec<u8>) -> Result<InstalledTheme, AppError> {
        let files = self.files.unpack(archive).await?;
        let manifest = check_package(&files, false)?;
        if manifest.id().is_builtin() {
            return Err(AppError::invalid(
                "theme-id-reserved",
                format!("{} is the name of a built-in theme", manifest.id()),
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
        write.commit().await?;
        self.signal.notify(&change);
        Ok(theme)
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
        let now = self.clock.now();
        let mut write = self.repository.begin().await?;
        let Some(change) = write.delete(&theme, expected, now).await? else {
            return Err(theme_not_found(id));
        };
        let mut changes = vec![change];
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

    /// A file of an installed theme; anything unsafe or missing is simply not found.
    pub async fn asset(&self, id: &str, path: &str) -> Result<Asset, AppError> {
        let theme = ThemeId::parse(id).map_err(|_| file_not_found())?;
        let segments = asset_segments(path).map_err(|_| file_not_found())?;
        let segments: Vec<String> = segments.into_iter().map(str::to_owned).collect();
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
    }

    /// Packages are archives of the form `path=text` lines in these tests.
    #[derive(Default)]
    pub(crate) struct MemoryFiles {
        stored: Mutex<HashMap<String, Vec<PackageFile>>>,
        removals: Mutex<Vec<String>>,
        fail_remove: Mutex<bool>,
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
        let theme = fixture.service.install(night_blue()).await.unwrap();
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
        fixture.service.install(night_blue()).await.unwrap();
        let mut files = package();
        files[0] = file("manifest.json", &MANIFEST.replace("1.0.0", "1.1.0"));
        let again = fixture.service.install(archive(&files)).await.unwrap();
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
        let error = fixture.service.install(archive(&files)).await.unwrap_err();
        assert_eq!(error.code(), "theme-package-invalid");
        let broken = vec![file("manifest.json", "{"), file("theme.css", "a {}")];
        let error = fixture.service.install(archive(&broken)).await.unwrap_err();
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
        let error = fixture.service.install(archive(&files)).await.unwrap_err();
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
        fixture.service.install(night_blue()).await.unwrap();
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
        let theme = fixture.service.install(night_blue()).await.unwrap();
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
        let theme = fixture.service.install(night_blue()).await.unwrap();
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
        fixture.service.install(night_blue()).await.unwrap();
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
        let theme = fixture.service.install(night_blue()).await.unwrap();
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
        fixture.service.install(night_blue()).await.unwrap();
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
}
