//! Cached files per owner below one folder, kept under a budget of bytes and files.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const SUFFIX: &str = "bin";
const STAGING_SUFFIX: &str = "tmp";

/// A staging file this old belongs to a writer that died.
const STAGING_MAX_AGE: Duration = Duration::from_hours(1);

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

/// What an owner may keep; a small file counts for at least [`FILE_COST`] bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheBudget {
    pub bytes: u64,
    pub files: usize,
}

/// Least a file counts for, so tiny bodies under many addresses cannot pile up.
pub const FILE_COST: u64 = 4096;

fn cost(len: u64) -> u64 {
    len.max(FILE_COST)
}

/// Keeps `bytes` under `key` and drops the oldest files of the owner until `budget` holds.
pub fn cache_write(
    dir: &Path,
    owner: &str,
    key: &str,
    bytes: &[u8],
    budget: CacheBudget,
) -> io::Result<()> {
    let path = location(dir, owner, key)?;
    if cost(bytes.len() as u64) > budget.bytes || budget.files == 0 {
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

/// Removes a staging file that no writer can still be filling.
fn drop_stale_staging(path: &Path, metadata: &fs::Metadata) {
    let stale = metadata.is_file()
        && path
            .extension()
            .is_some_and(|suffix| suffix == STAGING_SUFFIX)
        && metadata
            .modified()
            .ok()
            .and_then(|written| SystemTime::now().duration_since(written).ok())
            .is_some_and(|age| age > STAGING_MAX_AGE);
    if stale {
        // a writer that is alive after all finds its file gone and fails, which is its own error
        let _ = fs::remove_file(path);
    }
}

/// Removes the oldest files, but never `keep`, until the kept files fit `budget`.
fn evict(folder: &Path, keep: &Path, budget: CacheBudget) -> io::Result<()> {
    let mut files: Vec<(SystemTime, PathBuf, u64)> = Vec::new();
    for child in fs::read_dir(folder)? {
        let child = child?;
        let Ok(metadata) = child.metadata() else {
            continue;
        };
        // staging files of writers that are not done are not kept files yet
        if child
            .path()
            .extension()
            .is_none_or(|suffix| suffix != SUFFIX)
        {
            drop_stale_staging(&child.path(), &metadata);
            continue;
        }
        if metadata.is_file() {
            let written = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((written, child.path(), cost(metadata.len())));
        }
    }
    let mut total: u64 = files.iter().map(|(_, _, cost)| cost).sum();
    let mut count = files.len();
    files.sort();
    for (_, path, cost) in files {
        if total <= budget.bytes && count <= budget.files {
            break;
        }
        if path == keep {
            continue;
        }
        // a concurrent writer may have removed it already
        if fs::remove_file(&path).is_ok() {
            total -= cost;
            count -= 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef";
    const BIG: usize = 4096;
    const _: () = assert!(BIG as u64 == FILE_COST);

    fn budget(bytes: u64, files: usize) -> CacheBudget {
        CacheBudget { bytes, files }
    }

    const ROOMY: CacheBudget = CacheBudget {
        bytes: 1 << 30,
        files: 1000,
    };

    fn write(dir: &Path, owner: &str, key: &str, len: usize, budget: CacheBudget) {
        cache_write(dir, owner, key, &vec![b'x'; len], budget).unwrap();
    }

    fn kept(dir: &Path, owner: &str) -> Vec<String> {
        let Ok(children) = fs::read_dir(dir.join(owner)) else {
            return Vec::new();
        };
        let mut names: Vec<String> = children
            .map(|child| child.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn set_written(path: &Path, written: SystemTime) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(written).unwrap();
    }

    fn age(dir: &Path, owner: &str, key: &str, seconds: u64) {
        let written = SystemTime::now() - Duration::from_secs(seconds);
        set_written(&dir.join(owner).join(format!("{key}.bin")), written);
    }

    #[test]
    fn what_was_written_is_read_back_and_only_within_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(cache_read(dir.path(), "night", KEY, 8).unwrap(), None);
        cache_write(dir.path(), "night", KEY, b"12345678", ROOMY).unwrap();
        assert_eq!(
            cache_read(dir.path(), "night", KEY, 8).unwrap(),
            Some(b"12345678".to_vec())
        );
        assert_eq!(cache_read(dir.path(), "night", KEY, 7).unwrap(), None);
        assert_eq!(cache_read(dir.path(), "other", KEY, 8).unwrap(), None);
        cache_write(dir.path(), "night", KEY, b"new", ROOMY).unwrap();
        assert_eq!(
            cache_read(dir.path(), "night", KEY, 8).unwrap(),
            Some(b"new".to_vec())
        );
        assert_eq!(kept(dir.path(), "night").len(), 1);
    }

    #[test]
    fn the_oldest_files_go_first_when_the_byte_budget_is_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let three = budget(3 * FILE_COST, 100);
        for (key, seconds) in [("a", 30), ("b", 20), ("c", 10)] {
            write(dir.path(), "night", key, BIG, three);
            age(dir.path(), "night", key, seconds);
        }
        write(dir.path(), "night", "d", BIG, three);
        assert_eq!(kept(dir.path(), "night"), ["b.bin", "c.bin", "d.bin"]);
        // the budget is per owner
        write(dir.path(), "other", "a", BIG, three);
        assert_eq!(kept(dir.path(), "night").len(), 3);
    }

    #[test]
    fn a_small_file_counts_for_at_least_four_kib() {
        let dir = tempfile::tempdir().unwrap();
        let two = budget(2 * FILE_COST + 10, 100);
        for (key, seconds) in [("a", 30), ("b", 20)] {
            write(dir.path(), "night", key, 1, two);
            age(dir.path(), "night", key, seconds);
        }
        write(dir.path(), "night", "c", 1, two);
        assert_eq!(kept(dir.path(), "night"), ["b.bin", "c.bin"]);
    }

    #[test]
    fn the_oldest_files_go_first_when_the_file_cap_is_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let cap = 4;
        let few = budget(1 << 30, cap);
        for n in 0..cap {
            write(dir.path(), "night", &format!("k{n}"), 1, few);
            age(dir.path(), "night", &format!("k{n}"), 100 - n as u64);
        }
        assert_eq!(kept(dir.path(), "night").len(), cap);
        // one above the cap drops exactly the oldest
        write(dir.path(), "night", "new", 1, few);
        assert_eq!(
            kept(dir.path(), "night"),
            ["k1.bin", "k2.bin", "k3.bin", "new.bin"]
        );
        // a cap of nothing keeps nothing
        write(dir.path(), "empty", "a", 1, budget(1 << 30, 0));
        assert_eq!(kept(dir.path(), "empty"), Vec::<String>::new());
    }

    #[test]
    fn staging_files_of_other_writers_are_not_counted_or_removed() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("night");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.1234-0.tmp"), vec![b'x'; 3 * BIG]).unwrap();
        let two = budget(2 * FILE_COST, 2);
        write(dir.path(), "night", "a", 1, two);
        write(dir.path(), "night", "b", 1, two);
        assert_eq!(
            kept(dir.path(), "night"),
            ["a.1234-0.tmp", "a.bin", "b.bin"]
        );
    }

    #[test]
    fn staging_files_older_than_an_hour_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("night");
        fs::create_dir_all(&folder).unwrap();
        let old = folder.join("a.1-0.tmp");
        let recent = folder.join("b.2-0.tmp");
        fs::write(&old, b"x").unwrap();
        fs::write(&recent, b"x").unwrap();
        let hour = Duration::from_hours(1);
        set_written(&old, SystemTime::now() - hour - Duration::from_secs(60));
        set_written(&recent, SystemTime::now() - hour + Duration::from_secs(60));
        write(dir.path(), "night", "a", 1, ROOMY);
        assert_eq!(kept(dir.path(), "night"), ["a.bin", "b.2-0.tmp"]);
    }

    #[test]
    fn the_file_just_written_stays_even_when_another_looks_newer() {
        let dir = tempfile::tempdir().unwrap();
        let one = budget(FILE_COST, 100);
        write(dir.path(), "night", "a", BIG, one);
        let later = SystemTime::now() + Duration::from_hours(1);
        set_written(&dir.path().join("night").join("a.bin"), later);
        write(dir.path(), "night", "b", BIG, one);
        assert_eq!(kept(dir.path(), "night"), ["b.bin"]);
    }

    #[test]
    fn a_file_above_the_budget_is_not_kept_and_the_new_file_is_never_evicted() {
        let dir = tempfile::tempdir().unwrap();
        let one = budget(FILE_COST, 100);
        write(dir.path(), "night", "a", BIG + 1, one);
        assert_eq!(cache_read(dir.path(), "night", "a", 1 << 20).unwrap(), None);
        write(dir.path(), "night", "b", BIG, one);
        write(dir.path(), "night", "c", BIG, one);
        assert_eq!(cache_read(dir.path(), "night", "b", 1 << 20).unwrap(), None);
        assert!(
            cache_read(dir.path(), "night", "c", 1 << 20)
                .unwrap()
                .is_some()
        );
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
                cache_write(dir.path(), owner, key, b"x", ROOMY).is_err(),
                "{owner} {key}"
            );
        }
        assert!(cache_clear(dir.path(), "..").is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn clearing_drops_one_owner_and_a_missing_one_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "night", KEY, 1, ROOMY);
        write(dir.path(), "other", KEY, 1, ROOMY);
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
