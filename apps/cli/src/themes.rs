//! The folder of installed themes: the files behind the app's `ThemeFiles` port.

use std::path::PathBuf;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::theme_package::{PackageFile, package_invalid};
use nullradix_app::themes::{ThemeFiles, files_failed};
use nullradix_domain::{MAX_ENTRIES, MAX_FILE_BYTES, MAX_PACKAGE_BYTES, ThemeId};
use nullradix_os::{ArchiveError, Entry, Limits};

pub(crate) const LIMITS: Limits = Limits {
    archive: MAX_PACKAGE_BYTES,
    file: MAX_FILE_BYTES,
    entries: MAX_ENTRIES,
};

/// Themes below one folder, one subfolder per theme id.
pub(crate) struct FolderThemes {
    dir: PathBuf,
    /// Cached resources of themes; a theme whose row changed has its part emptied.
    cache: Option<PathBuf>,
}

impl FolderThemes {
    pub(crate) const fn new(dir: PathBuf) -> Self {
        Self { dir, cache: None }
    }

    pub(crate) fn with_cache(mut self, cache: PathBuf) -> Self {
        self.cache = Some(cache);
        self
    }

    /// The cache is only a copy, so a folder that cannot be emptied is logged and left.
    async fn clear_cache(&self, id: &ThemeId) {
        let Some(cache) = self.cache.clone() else {
            return;
        };
        let name = id.to_string();
        let cleared =
            tokio::task::spawn_blocking(move || nullradix_os::cache_clear(&cache, &name)).await;
        if let Ok(Err(error)) = cleared {
            tracing::warn!(%error, theme = %id, "cannot empty the cache of the theme");
        }
    }
}

/// A refused package is the caller's problem; anything else is the folder's.
pub(crate) fn refused(error: ArchiveError) -> AppError {
    match error {
        ArchiveError::Io(error) => files_failed(error),
        other => package_invalid(other.to_string()),
    }
}

async fn blocking<T, F>(work: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(files_failed)?
}

#[async_trait]
impl ThemeFiles for FolderThemes {
    async fn unpack(&self, archive: Vec<u8>) -> Result<Vec<PackageFile>, AppError> {
        blocking(move || {
            let entries = nullradix_os::unpack_zip(&archive, LIMITS).map_err(refused)?;
            Ok(entries
                .into_iter()
                .map(|entry| PackageFile {
                    path: entry.path,
                    data: entry.data,
                })
                .collect())
        })
        .await
    }

    async fn store(&self, id: &ThemeId, files: Vec<PackageFile>) -> Result<(), AppError> {
        let (dir, name) = (self.dir.clone(), id.to_string());
        blocking(move || {
            let entries: Vec<Entry> = files
                .into_iter()
                .map(|file| Entry {
                    path: file.path,
                    data: file.data,
                })
                .collect();
            nullradix_os::replace_dir(&dir, &name, &entries).map_err(refused)
        })
        .await
    }

    async fn remove(&self, id: &ThemeId) -> Result<(), AppError> {
        let (dir, name) = (self.dir.clone(), id.to_string());
        blocking(move || {
            nullradix_os::remove_dir(&dir, &name)
                .map(|_| ())
                .map_err(refused)
        })
        .await
    }

    async fn read(&self, id: &ThemeId, path: &[String]) -> Result<Option<Vec<u8>>, AppError> {
        let (root, path) = (self.dir.join(id.as_str()), path.to_vec());
        blocking(move || {
            nullradix_os::read_within(&root, &path, MAX_FILE_BYTES).map_err(files_failed)
        })
        .await
    }

    async fn settled(&self, id: &ThemeId) {
        self.clear_cache(id).await;
    }

    async fn paths(&self, id: &ThemeId) -> Option<Vec<String>> {
        let root = self.dir.join(id.as_str());
        tokio::task::spawn_blocking(move || nullradix_os::list_files(&root, LIMITS.entries))
            .await
            .ok()
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nullradix_app::themes::ThemeService;
    use nullradix_app::{ChangeSignal, SystemClock, UuidV7Generator};
    use nullradix_store::Store;

    use super::*;

    const MANIFEST: &str = r#"{
        "id": "night-blue", "name": "Night Blue", "version": "1.0.0", "author": "Ada",
        "sdkVersion": "^2.0", "colorScheme": "dark", "slots": [], "permissions": { "network": [] }
    }"#;

    fn entry(path: &str, data: &str) -> Entry {
        Entry {
            path: path.to_owned(),
            data: data.as_bytes().to_vec(),
        }
    }

    async fn service(dir: PathBuf) -> ThemeService {
        ThemeService::new(
            Arc::new(Store::in_memory().await.unwrap()),
            Arc::new(FolderThemes::new(dir)),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            ChangeSignal::new(),
        )
    }

    fn package(extra: &[Entry]) -> Vec<u8> {
        let mut files = vec![
            entry("manifest.json", MANIFEST),
            entry("theme.css", "a { color: red }"),
        ];
        files.extend(extra.iter().cloned());
        nullradix_os::pack_zip(&files).unwrap()
    }

    #[tokio::test]
    async fn a_zip_package_is_installed_into_its_folder_and_served_from_there() {
        let dir = tempfile::tempdir().unwrap();
        let themes = service(dir.path().join("themes")).await;
        themes
            .install(package(&[entry("assets/a.png", "png")]), false, true)
            .await
            .unwrap();
        assert!(dir.path().join("themes/night-blue/theme.css").is_file());
        let css = themes.asset("night-blue", "theme.css").await.unwrap();
        assert_eq!(css.data, b"a { color: red }");
        let png = themes.asset("night-blue", "assets/a.png").await.unwrap();
        assert_eq!(png.content_type, "image/png");
        for path in [
            "../night-blue/theme.css",
            "assets/../theme.css",
            "/theme.css",
            "assets",
        ] {
            let error = themes.asset("night-blue", path).await.unwrap_err();
            assert_eq!(error.code(), "theme-file-not-found", "{path}");
        }

        let list = themes.list().await.unwrap();
        themes
            .remove("night-blue", list.themes[0].record().version)
            .await
            .unwrap();
        assert!(!dir.path().join("themes/night-blue").exists());
    }

    #[tokio::test]
    async fn a_theme_whose_row_changed_has_its_cache_emptied() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let files = FolderThemes::new(dir.path().join("themes")).with_cache(cache.clone());
        let id = ThemeId::parse("night-blue").unwrap();
        let kept = |theme: &str| cache.join(theme).join("ab12.bin");
        let budget = nullradix_os::CacheBudget {
            bytes: 1 << 20,
            files: 10,
        };
        for theme in ["night-blue", "other-theme"] {
            nullradix_os::cache_write(&cache, theme, "ab12", b"x", budget).unwrap();
        }
        // changing the files alone leaves the cache; the row decides
        let package = vec![PackageFile {
            path: String::from("theme.css"),
            data: b"a{}".to_vec(),
        }];
        files.store(&id, package).await.unwrap();
        assert!(kept("night-blue").exists());
        files.settled(&id).await;
        assert!(!kept("night-blue").exists() && kept("other-theme").exists());
    }

    #[tokio::test]
    async fn hostile_archives_are_refused_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let themes = service(dir.path().join("themes")).await;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("manifest.json", options).unwrap();
        std::io::Write::write_all(&mut writer, MANIFEST.as_bytes()).unwrap();
        writer.start_file("theme.css", options).unwrap();
        writer.start_file("../../evil.css", options).unwrap();
        let traversal = writer.finish().unwrap().into_inner();
        let error = themes.install(traversal, false, true).await.unwrap_err();
        assert_eq!(error.code(), "theme-package-invalid");

        let error = themes
            .install(b"not a zip".to_vec(), false, true)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-package-invalid");
        let error = themes
            .install(package(&[entry("script.sh", "rm -rf")]), false, true)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-package-invalid");
        let themes_dir = dir.path().join("themes");
        assert!(!themes_dir.exists() || std::fs::read_dir(&themes_dir).unwrap().next().is_none());
        assert!(!dir.path().join("evil.css").exists());
    }

    #[tokio::test]
    async fn a_new_package_replaces_the_files_of_the_installed_one() {
        let dir = tempfile::tempdir().unwrap();
        let themes = service(dir.path().join("themes")).await;
        themes
            .install(package(&[entry("old.css", "old")]), false, true)
            .await
            .unwrap();
        themes
            .install(package(&[entry("new.css", "new")]), false, true)
            .await
            .unwrap();
        assert!(!dir.path().join("themes/night-blue/old.css").exists());
        assert!(dir.path().join("themes/night-blue/new.css").exists());
    }

    #[tokio::test]
    async fn an_update_adding_code_to_an_installed_folder_needs_a_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let themes = service(dir.path().join("themes")).await;
        let files = FolderThemes::new(dir.path().join("themes"));
        let id = ThemeId::parse("night-blue").unwrap();
        assert_eq!(files.paths(&id).await, Some(Vec::new()));
        themes.install(package(&[]), false, true).await.unwrap();
        assert_eq!(
            files.paths(&id).await.unwrap(),
            ["manifest.json", "theme.css"]
        );
        let error = themes
            .install(package(&[entry("main.js", "run()")]), false, true)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-confirmation-required");
        let done = themes
            .install(package(&[entry("main.js", "run()")]), true, true)
            .await
            .unwrap();
        assert!(done.added.code);
        let error = themes
            .install(package(&[entry("main.js", "run()")]), false, true)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-confirmation-required");
        let same = themes
            .install(package(&[entry("main.js", "run()")]), true, true)
            .await
            .unwrap();
        assert!(same.added.is_empty());
    }

    #[tokio::test]
    async fn stray_files_in_the_installed_folder_never_block_a_reinstall() {
        let dir = tempfile::tempdir().unwrap();
        let themes = service(dir.path().join("themes")).await;
        let folder = dir.path().join("themes/night-blue");
        let code = package(&[entry("main.js", "run()")]);
        themes.install(code.clone(), true, true).await.unwrap();
        std::fs::write(folder.join(".DS_Store"), "x").unwrap();
        let same = themes.install(code.clone(), true, true).await.unwrap();
        assert!(same.added.is_empty());
        let error = themes.install(code.clone(), false, true).await.unwrap_err();
        assert_eq!(error.code(), "theme-confirmation-required");

        // too many files: the old risk is unknown, so code counts as added
        let crowd = folder.join("crowd");
        std::fs::create_dir(&crowd).unwrap();
        for n in 0..=MAX_ENTRIES {
            std::fs::write(crowd.join(format!("{n}.css")), "").unwrap();
        }
        let repaired = themes.install(code, true, true).await.unwrap();
        assert!(repaired.added.code);
    }

    #[tokio::test]
    async fn a_package_at_the_file_cap_with_a_subfolder_reinstalls_without_adding_code() {
        let dir = tempfile::tempdir().unwrap();
        let themes = service(dir.path().join("themes")).await;
        // manifest, style and script plus the rest fill exactly the cap
        let mut files = vec![entry("main.js", "run()")];
        files.extend((0..MAX_ENTRIES - 3).map(|n| entry(&format!("assets/{n}.css"), "")));
        let full = package(&files);
        themes.install(full.clone(), true, true).await.unwrap();
        let again = themes.install(full, true, true).await.unwrap();
        assert!(again.added.is_empty());
    }

    #[tokio::test]
    async fn code_in_a_hidden_path_is_not_added_again_by_an_update() {
        let dir = tempfile::tempdir().unwrap();
        let themes = service(dir.path().join("themes")).await;
        let hidden = package(&[entry(".cache/run.js", "run()")]);
        let first = themes.install(hidden.clone(), true, true).await.unwrap();
        assert!(first.added.code);
        let again = themes.install(hidden, true, true).await.unwrap();
        assert!(again.added.is_empty());
    }
}
