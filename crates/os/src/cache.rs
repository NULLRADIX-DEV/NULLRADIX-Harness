//! Cached files per owner below one folder, kept under a size budget.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const SUFFIX: &str = "bin";

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("not a plain {what}"))
}

/// Owner and key become file names, so only letters, digits and `-` pass.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn location(dir: &Path, owner: &str, key: &str) -> io::Result<PathBuf> {
    if !plain(owner) {
        return Err(invalid("owner"));
    }
    if !plain(key) {
        return Err(invalid("key"));
    }
    Ok(dir.join(owner).join(format!("{key}.{SUFFIX}")))
}

/// The file kept under `key`; `None` when there is none, it is no regular file or above `max`.
pub fn cache_read(dir: &Path, owner: &str, key: &str, max: u64) -> io::Result<Option<Vec<u8>>> {
    let path = location(dir, owner, key)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.len() > max {
        return Ok(None);
    }
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        // evicted since the check above
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut data = Vec::new();
    file.take(max + 1).read_to_end(&mut data)?;
    Ok((data.len() as u64 <= max).then_some(data))
}

/// Keeps `bytes` under `key` and drops the oldest files of the owner until `budget` holds.
pub fn cache_write(
    dir: &Path,
    owner: &str,
    key: &str,
    bytes: &[u8],
    budget: u64,
) -> io::Result<()> {
    let path = location(dir, owner, key)?;
    if bytes.len() as u64 > budget {
        return Ok(());
    }
    let folder = dir.join(owner);
    fs::create_dir_all(&folder)?;
    crate::write_atomic(&path, bytes)?;
    evict(&folder, &path, budget)
}

/// Drops everything kept for the owner.
pub fn cache_clear(dir: &Path, owner: &str) -> io::Result<()> {
    if !plain(owner) {
        return Err(invalid("owner"));
    }
    match fs::remove_dir_all(dir.join(owner)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Removes the oldest files, but never `keep`, until the files of the folder fit `budget`.
fn evict(folder: &Path, keep: &Path, budget: u64) -> io::Result<()> {
    let mut files: Vec<(SystemTime, PathBuf, u64)> = Vec::new();
    for child in fs::read_dir(folder)? {
        let child = child?;
        let Ok(metadata) = child.metadata() else {
            continue;
        };
        if metadata.is_file() {
            let written = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((written, child.path(), metadata.len()));
        }
    }
    let mut total: u64 = files.iter().map(|(_, _, len)| len).sum();
    files.sort();
    for (_, path, len) in files {
        if total <= budget {
            break;
        }
        if path == keep {
            continue;
        }
        // a concurrent writer may have removed it already
        if fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef";

    #[test]
    fn what_was_written_is_read_back_and_only_within_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(cache_read(dir.path(), "night", KEY, 8).unwrap(), None);
        cache_write(dir.path(), "night", KEY, b"12345678", 100).unwrap();
        assert_eq!(
            cache_read(dir.path(), "night", KEY, 8).unwrap(),
            Some(b"12345678".to_vec())
        );
        assert_eq!(cache_read(dir.path(), "night", KEY, 7).unwrap(), None);
        assert_eq!(cache_read(dir.path(), "other", KEY, 8).unwrap(), None);
        cache_write(dir.path(), "night", KEY, b"new", 100).unwrap();
        assert_eq!(
            cache_read(dir.path(), "night", KEY, 8).unwrap(),
            Some(b"new".to_vec())
        );
        let names: Vec<_> = fs::read_dir(dir.path().join("night"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn the_oldest_files_go_first_when_the_budget_is_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("night");
        for (key, age) in [("a", 30), ("b", 20), ("c", 10)] {
            cache_write(dir.path(), "night", key, b"1234", 100).unwrap();
            let written = SystemTime::now() - std::time::Duration::from_secs(age);
            let file = fs::File::options()
                .write(true)
                .open(folder.join(format!("{key}.bin")))
                .unwrap();
            file.set_modified(written).unwrap();
        }
        // 12 bytes are kept; a fourth of 4 bytes under a budget of 12 drops the oldest
        cache_write(dir.path(), "night", "d", b"1234", 12).unwrap();
        let mut left: Vec<String> = fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["b.bin", "c.bin", "d.bin"]);
        // the budget is per owner
        cache_write(dir.path(), "other", "a", b"1234", 12).unwrap();
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 3);
    }

    #[test]
    fn a_file_above_the_budget_is_not_kept_and_the_new_file_is_never_evicted() {
        let dir = tempfile::tempdir().unwrap();
        cache_write(dir.path(), "night", "a", b"12345", 4).unwrap();
        assert_eq!(cache_read(dir.path(), "night", "a", 100).unwrap(), None);
        cache_write(dir.path(), "night", "b", b"1234", 4).unwrap();
        cache_write(dir.path(), "night", "c", b"1234", 4).unwrap();
        assert_eq!(cache_read(dir.path(), "night", "b", 100).unwrap(), None);
        assert!(cache_read(dir.path(), "night", "c", 100).unwrap().is_some());
    }

    #[test]
    fn names_that_are_not_plain_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        for (owner, key) in [
            ("..", KEY),
            ("a/b", KEY),
            ("a\\b", KEY),
            ("", KEY),
            ("night", ".."),
            ("night", "a/b"),
            ("night", "a.b"),
            ("night", ""),
            ("night", &"a".repeat(129)),
        ] {
            assert!(
                cache_read(dir.path(), owner, key, 8).is_err(),
                "{owner} {key}"
            );
            assert!(
                cache_write(dir.path(), owner, key, b"x", 8).is_err(),
                "{owner} {key}"
            );
        }
        assert!(cache_clear(dir.path(), "..").is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn clearing_drops_one_owner_and_a_missing_one_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        cache_write(dir.path(), "night", KEY, b"x", 8).unwrap();
        cache_write(dir.path(), "other", KEY, b"y", 8).unwrap();
        cache_clear(dir.path(), "night").unwrap();
        cache_clear(dir.path(), "night").unwrap();
        assert_eq!(cache_read(dir.path(), "night", KEY, 8).unwrap(), None);
        assert!(cache_read(dir.path(), "other", KEY, 8).unwrap().is_some());
    }

    #[test]
    fn a_folder_in_place_of_a_file_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("night").join(format!("{KEY}.bin"))).unwrap();
        assert_eq!(cache_read(dir.path(), "night", KEY, 8).unwrap(), None);
    }
}
