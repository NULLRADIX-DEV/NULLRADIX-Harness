//! Archives and the cache folder behind the asset proxy of themes.

use std::path::PathBuf;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::theme_fetch::{AssetCache, EntryError, ZipEntries};
use nullradix_app::themes::files_failed;
use nullradix_domain::{MAX_ARCHIVE_BYTES, MAX_CACHE_BYTES, ThemeId};
use nullradix_os::ArchiveError;

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

    async fn put(&self, theme: &ThemeId, key: &str, bytes: &[u8]) -> Result<(), AppError> {
        let (dir, theme, key, bytes) = (
            self.dir.clone(),
            theme.to_string(),
            key.to_owned(),
            bytes.to_vec(),
        );
        tokio::task::spawn_blocking(move || {
            nullradix_os::cache_write(&dir, &theme, &key, &bytes, MAX_CACHE_BYTES)
        })
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
        cache.put(&theme(), key, b"PNG").await.unwrap();
        assert_eq!(cache.get(&theme(), key).await, Some(b"PNG".to_vec()));
        let other = ThemeId::parse("other-theme").unwrap();
        assert_eq!(cache.get(&other, key).await, None);
        assert!(cache.put(&theme(), "../x", b"PNG").await.is_err());
        assert_eq!(cache.get(&theme(), "../x").await, None);
    }
}
