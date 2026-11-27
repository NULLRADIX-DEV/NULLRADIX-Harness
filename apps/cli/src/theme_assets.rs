//! Archives and the cache folder behind the asset proxy of themes.

use std::path::PathBuf;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::theme_fetch::{AssetCache, EntryError, Stored, ZipEntries};
use nullradix_app::theme_preview::PreviewCache;
use nullradix_app::themes::files_failed;
use nullradix_domain::{
    MAX_ARCHIVE_BYTES, MAX_CACHE_BYTES, MAX_CACHE_FILES, MAX_PREVIEW_BYTES,
    MAX_PREVIEW_CACHE_BYTES, MAX_PREVIEW_CACHE_FILES, SourceId, ThemeId,
};
use nullradix_os::{ArchiveError, CacheBudget};

/// Reads files out of zip archives held in memory.
pub(crate) struct ZipFiles;

fn entry_problem(error: &ArchiveError) -> EntryError {
    match error {
        ArchiveError::Missing(_) => EntryError::NotFound,
        ArchiveError::UnsafePath(_) | ArchiveError::Link(_) => EntryError::Unsafe,
        ArchiveError::FileTooLarge { .. } | ArchiveError::TooLarge { .. } => EntryError::TooLarge,
        _ => EntryError::NotArchive,
    }
}

#[async_trait]
impl ZipEntries for ZipFiles {
    async fn entry(
        &self,
        archive: Vec<u8>,
        path: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, EntryError> {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || {
            nullradix_os::read_zip_entry(&archive, &path, max_bytes)
        })
        .await
        .map_err(|_| EntryError::NotArchive)?
        .map_err(|error| entry_problem(&error))
    }
}

/// Cached resources below one folder, one subfolder per theme, kept under a budget per theme.
pub(crate) struct FolderAssets {
    dir: PathBuf,
}

impl FolderAssets {
    pub(crate) const fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

#[async_trait]
impl AssetCache for FolderAssets {
    async fn get(&self, theme: &ThemeId, key: &str) -> Option<Vec<u8>> {
        let (dir, theme, key) = (self.dir.clone(), theme.to_string(), key.to_owned());
        tokio::task::spawn_blocking(move || {
            nullradix_os::cache_read(&dir, &theme, &key, MAX_ARCHIVE_BYTES)
        })
        .await
        .ok()?
        .ok()?
    }

    async fn put(&self, theme: &ThemeId, key: &str, bytes: Vec<u8>) -> Result<Stored, AppError> {
        let (dir, theme, key) = (self.dir.clone(), theme.to_string(), key.to_owned());
        let budget = CacheBudget {
            bytes: MAX_CACHE_BYTES,
            files: MAX_CACHE_FILES,
        };
        // the bytes go to the thread and come back, so they are not copied
        let (bytes, written) = tokio::task::spawn_blocking(move || {
            let written = nullradix_os::cache_write(&dir, &theme, &key, &bytes, budget);
            (bytes, written)
        })
        .await
        .map_err(files_failed)?;
        Ok(Stored {
            bytes,
            error: written.err().map(files_failed),
        })
    }
}

/// Cached preview images below one folder, one subfolder per catalog source.
pub(crate) struct FolderPreviews {
    dir: PathBuf,
}

impl FolderPreviews {
    pub(crate) const fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

#[async_trait]
impl PreviewCache for FolderPreviews {
    async fn get(&self, source: &SourceId, key: &str) -> Option<Vec<u8>> {
        let (dir, source, key) = (self.dir.clone(), source.to_string(), key.to_owned());
        tokio::task::spawn_blocking(move || {
            nullradix_os::cache_read(&dir, &source, &key, MAX_PREVIEW_BYTES)
        })
        .await
        .ok()?
        .ok()?
    }

    async fn put(&self, source: &SourceId, key: &str, bytes: Vec<u8>) -> Result<Stored, AppError> {
        let (dir, source, key) = (self.dir.clone(), source.to_string(), key.to_owned());
        let budget = CacheBudget {
            bytes: MAX_PREVIEW_CACHE_BYTES,
            files: MAX_PREVIEW_CACHE_FILES,
        };
        // the bytes go to the thread and come back, so they are not copied
        let (bytes, written) = tokio::task::spawn_blocking(move || {
            let written = nullradix_os::cache_write(&dir, &source, &key, &bytes, budget);
            (bytes, written)
        })
        .await
        .map_err(files_failed)?;
        Ok(Stored {
            bytes,
            error: written.err().map(files_failed),
        })
    }

    async fn clear(&self, source: &SourceId) -> Result<(), AppError> {
        let (dir, source) = (self.dir.clone(), source.to_string());
        tokio::task::spawn_blocking(move || nullradix_os::cache_clear(&dir, &source))
            .await
            .map_err(files_failed)?
            .map_err(files_failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> ThemeId {
        ThemeId::parse("night-blue").unwrap()
    }

    #[tokio::test]
    async fn a_file_is_read_from_an_archive_and_problems_get_their_names() {
        let archive = nullradix_os::pack_zip(&[nullradix_os::Entry {
            path: String::from("a/b.png"),
            data: b"PNG".to_vec(),
        }])
        .unwrap();
        let read = |path: &'static str, max| {
            let archive = archive.clone();
            async move { ZipFiles.entry(archive, path, max).await }
        };
        assert_eq!(read("a/b.png", 3).await.unwrap(), b"PNG");
        assert_eq!(read("a/b.png", 2).await, Err(EntryError::TooLarge));
        assert_eq!(read("a/c.png", 3).await, Err(EntryError::NotFound));
        assert_eq!(read("a", 3).await, Err(EntryError::NotFound));
        let junk = ZipFiles.entry(b"junk".to_vec(), "a", 3).await;
        assert_eq!(junk, Err(EntryError::NotArchive));
    }

    #[tokio::test]
    async fn the_cache_keeps_bytes_per_theme_and_refuses_odd_keys() {
        let dir = tempfile::tempdir().unwrap();
        let cache = FolderAssets::new(dir.path().to_path_buf());
        let key = "ab12";
        assert_eq!(cache.get(&theme(), key).await, None);
        cache.put(&theme(), key, b"PNG".to_vec()).await.unwrap();
        assert_eq!(cache.get(&theme(), key).await, Some(b"PNG".to_vec()));
        let other = ThemeId::parse("other-theme").unwrap();
        assert_eq!(cache.get(&other, key).await, None);
        assert!(
            cache
                .put(&theme(), "../x", b"PNG".to_vec())
                .await
                .unwrap()
                .error
                .is_some()
        );
        assert_eq!(cache.get(&theme(), "../x").await, None);
    }

    fn source(text: &str) -> SourceId {
        SourceId::parse(text).unwrap()
    }

    #[tokio::test]
    async fn previews_are_kept_per_source_and_cleared_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let cache = FolderPreviews::new(dir.path().to_path_buf());
        let (one, two) = (source("aaaaaaaaaaaa"), source("bbbbbbbbbbbb"));
        assert_eq!(cache.get(&one, "ab12").await, None);
        cache.put(&one, "ab12", b"PNG".to_vec()).await.unwrap();
        cache.put(&two, "ab12", b"JPG".to_vec()).await.unwrap();
        assert_eq!(cache.get(&one, "ab12").await, Some(b"PNG".to_vec()));
        assert_eq!(cache.get(&two, "ab12").await, Some(b"JPG".to_vec()));
        cache.clear(&one).await.unwrap();
        assert_eq!(cache.get(&one, "ab12").await, None);
        assert_eq!(cache.get(&two, "ab12").await, Some(b"JPG".to_vec()));
        // clearing what is not there is no error
        cache.clear(&one).await.unwrap();
    }

    #[tokio::test]
    async fn the_preview_cache_refuses_odd_keys_and_serves_nothing_above_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let cache = FolderPreviews::new(dir.path().to_path_buf());
        let one = source("aaaaaaaaaaaa");
        let put = cache.put(&one, "../x", b"PNG".to_vec()).await.unwrap();
        assert!(put.error.is_some());
        assert_eq!(cache.get(&one, "../x").await, None);
        let budget = CacheBudget {
            bytes: 1 << 30,
            files: 10,
        };
        let big = vec![0_u8; usize::try_from(MAX_PREVIEW_BYTES).unwrap() + 1];
        nullradix_os::cache_write(dir.path(), "aaaaaaaaaaaa", "big", &big, budget).unwrap();
        assert_eq!(cache.get(&one, "big").await, None);
        let fits = vec![0_u8; usize::try_from(MAX_PREVIEW_BYTES).unwrap()];
        nullradix_os::cache_write(dir.path(), "aaaaaaaaaaaa", "fits", &fits, budget).unwrap();
        assert_eq!(
            cache.get(&one, "fits").await.map(|bytes| bytes.len()),
            Some(fits.len())
        );
    }
}
