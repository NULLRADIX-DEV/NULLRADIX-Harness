//! Zip archives and folders of files, read and written without leaving their folder.

use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};

/// A file of an archive or folder: a relative path with `/` and its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub data: Vec<u8>,
}

/// Largest archive, unpacked file and number of files accepted.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub archive: u64,
    pub file: u64,
    pub entries: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("not a zip archive: {0}")]
    NotArchive(String),
    #[error("larger than {max} bytes")]
    TooLarge { max: u64 },
    #[error("{path} is larger than {max} bytes")]
    FileTooLarge { path: String, max: u64 },
    #[error("more than {max} files")]
    TooManyEntries { max: usize },
    #[error("unsafe path: {0}")]
    UnsafePath(String),
    #[error("links are not allowed: {0}")]
    Link(String),
    #[error("hidden files are not allowed: {0}")]
    Hidden(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Names Windows keeps for devices; a file with such a stem cannot be written there.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

fn plain_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0'])
        && !name.chars().any(char::is_control)
        && !name.ends_with([' ', '.'])
        && !RESERVED.contains(&stem.as_str())
}

/// The path of an entry as a relative path of plain names, or `None`.
fn safe_relative(path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.starts_with('/') {
        return None;
    }
    let mut safe = PathBuf::new();
    for part in path.split('/') {
        if !plain_name(part) {
            return None;
        }
        safe.push(part);
    }
    // a path that Windows would read as rooted or with a drive has no plain names only
    safe.components()
        .all(|component| matches!(component, Component::Normal(_)))
        .then_some(safe)
}

fn unsafe_path(path: &str) -> ArchiveError {
    ArchiveError::UnsafePath(path.to_owned())
}

/// Reads every file of a zip archive into memory; folders are skipped.
pub fn unpack_zip(archive: &[u8], limits: Limits) -> Result<Vec<Entry>, ArchiveError> {
    if archive.len() as u64 > limits.archive {
        return Err(ArchiveError::TooLarge {
            max: limits.archive,
        });
    }
    let not_zip = |error: zip::result::ZipError| ArchiveError::NotArchive(error.to_string());
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).map_err(not_zip)?;
    if zip.len() > limits.entries {
        return Err(ArchiveError::TooManyEntries {
            max: limits.entries,
        });
    }
    let mut entries: Vec<Entry> = Vec::new();
    // names are compared lowercased: case-insensitive disks would merge them
    let mut seen = std::collections::HashSet::new();
    let mut total: u64 = 0;
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).map_err(not_zip)?;
        let name = file.name().map_err(not_zip)?.to_string();
        if file.is_symlink() {
            return Err(ArchiveError::Link(name));
        }
        if file.enclosed_name().is_none() {
            return Err(unsafe_path(&name));
        }
        let trimmed = name.strip_suffix('/').unwrap_or(&name);
        if safe_relative(trimmed).is_none() {
            return Err(unsafe_path(&name));
        }
        if file.is_dir() {
            continue;
        }
        let too_large = || ArchiveError::FileTooLarge {
            path: name.clone(),
            max: limits.file,
        };
        if file.size() > limits.file {
            return Err(too_large());
        }
        // the stated size can lie, so read one byte more than allowed
        let mut data = Vec::new();
        (&mut file)
            .take(limits.file + 1)
            .read_to_end(&mut data)
            .map_err(|error| ArchiveError::NotArchive(error.to_string()))?;
        if data.len() as u64 > limits.file {
            return Err(too_large());
        }
        total += data.len() as u64;
        if total > limits.archive {
            return Err(ArchiveError::TooLarge {
                max: limits.archive,
            });
        }
        if !seen.insert(trimmed.to_lowercase()) {
            return Err(unsafe_path(&name));
        }
        entries.push(Entry {
            path: trimmed.to_owned(),
            data,
        });
    }
    Ok(entries)
}

/// A zip archive of the files; the same files always give the same bytes.
pub fn pack_zip(entries: &[Entry]) -> Result<Vec<u8>, ArchiveError> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    // a fixed time and system keep the archive equal on every platform
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default())
        .system(zip::System::Unix)
        .unix_permissions(0o644);
    let not_zip = |error: zip::result::ZipError| ArchiveError::NotArchive(error.to_string());
    for entry in entries {
        writer
            .start_file(entry.path.as_str(), options)
            .map_err(not_zip)?;
        writer.write_all(&entry.data)?;
    }
    Ok(writer.finish().map_err(not_zip)?.into_inner())
}

/// Every file below `dir` by path; links, hidden names and files leaving `dir` are refused.
pub fn read_tree(dir: &Path, limits: Limits) -> Result<Vec<Entry>, ArchiveError> {
    let root = fs::canonicalize(dir)?;
    let mut entries = Vec::new();
    let mut total = 0;
    collect(&root, &root, "", limits, &mut total, &mut entries)?;
    Ok(entries)
}

fn collect(
    root: &Path,
    dir: &Path,
    prefix: &str,
    limits: Limits,
    total: &mut u64,
    entries: &mut Vec<Entry>,
) -> Result<(), ArchiveError> {
    let mut children = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(fs::DirEntry::file_name);
    for child in children {
        let name = child.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}{name}");
        let kind = child.file_type()?;
        if kind.is_symlink() {
            return Err(ArchiveError::Link(path));
        }
        if name.starts_with('.') {
            return Err(ArchiveError::Hidden(path));
        }
        if !plain_name(&name) || !fs::canonicalize(child.path())?.starts_with(root) {
            return Err(unsafe_path(&path));
        }
        if kind.is_dir() {
            collect(
                root,
                &child.path(),
                &format!("{path}/"),
                limits,
                total,
                entries,
            )?;
            continue;
        }
        if entries.len() >= limits.entries {
            return Err(ArchiveError::TooManyEntries {
                max: limits.entries,
            });
        }
        if child.metadata()?.len() > limits.file {
            return Err(ArchiveError::FileTooLarge {
                path,
                max: limits.file,
            });
        }
        let data = fs::read(child.path())?;
        *total += data.len() as u64;
        if *total > limits.archive {
            return Err(ArchiveError::TooLarge {
                max: limits.archive,
            });
        }
        entries.push(Entry { path, data });
    }
    Ok(())
}

/// Makes `parent/name` hold exactly `entries`; the old folder stays until the new one is complete.
pub fn replace_dir(parent: &Path, name: &str, entries: &[Entry]) -> Result<(), ArchiveError> {
    if !plain_name(name) {
        return Err(unsafe_path(name));
    }
    let relative = entries
        .iter()
        .map(|entry| safe_relative(&entry.path).ok_or_else(|| unsafe_path(&entry.path)))
        .collect::<Result<Vec<_>, _>>()?;
    fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".tmp-")
        .tempdir_in(parent)?;
    for (entry, path) in entries.iter().zip(relative) {
        let target = staging.path().join(path);
        if let Some(folder) = target.parent() {
            fs::create_dir_all(folder)?;
        }
        fs::write(target, &entry.data)?;
    }
    let target = parent.join(name);
    let old = if target.exists() {
        let old = tempfile::Builder::new()
            .prefix(".old-")
            .tempdir_in(parent)?;
        fs::rename(&target, old.path().join("previous"))?;
        Some(old)
    } else {
        None
    };
    if let Err(error) = fs::rename(staging.path(), &target) {
        if let Some(old) = old {
            // the restore error would hide the cause; the old folder stays kept either way
            let _ = restore_previous(old, &target);
        }
        return Err(error.into());
    }
    // the staging folder is now the theme; only the displaced old one is deleted on drop
    let _ = staging.keep();
    Ok(())
}

/// Moves the displaced folder back; when that fails it is kept, not deleted with `old`.
fn restore_previous(old: tempfile::TempDir, target: &Path) -> io::Result<()> {
    let kept = old.keep();
    fs::rename(kept.join("previous"), target)?;
    let _ = fs::remove_dir_all(kept);
    Ok(())
}

/// Deletes `parent/name`; `false` when there was none.
pub fn remove_dir(parent: &Path, name: &str) -> Result<bool, ArchiveError> {
    if !plain_name(name) {
        return Err(unsafe_path(name));
    }
    match fs::remove_dir_all(parent.join(name)) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// The regular file at `path` below `root`; `None` when missing, too large or outside `root`.
pub fn read_within(root: &Path, path: &[String], max: u64) -> io::Result<Option<Vec<u8>>> {
    let mut file = root.to_path_buf();
    for part in path {
        if !plain_name(part) {
            return Ok(None);
        }
        file.push(part);
    }
    let (Some(root), Some(target)) = (resolve(root)?, resolve(&file)?) else {
        return Ok(None);
    };
    if !target.starts_with(&root) {
        return Ok(None);
    }
    // the file may vanish between the checks, which is a miss, not a failure
    let Some(metadata) = missing_as_none(fs::metadata(&target))? else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    let Some(file) = missing_as_none(fs::File::open(&target))? else {
        return Ok(None);
    };
    let mut data = Vec::new();
    file.take(max + 1).read_to_end(&mut data)?;
    Ok((data.len() as u64 <= max).then_some(data))
}

fn missing_as_none<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn resolve(path: &Path) -> io::Result<Option<PathBuf>> {
    missing_as_none(fs::canonicalize(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        archive: 1024,
        file: 256,
        entries: 8,
    };

    fn entry(path: &str, data: &str) -> Entry {
        Entry {
            path: path.to_owned(),
            data: data.as_bytes().to_vec(),
        }
    }

    fn raw_zip(items: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in items {
            if name.ends_with('/') {
                writer.add_directory(*name, options).unwrap();
            } else {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn packing_and_unpacking_round_trip_and_packing_is_deterministic() {
        let files = [entry("manifest.json", "{}"), entry("assets/a.png", "png")];
        let bytes = pack_zip(&files).unwrap();
        assert_eq!(pack_zip(&files).unwrap(), bytes);
        assert_eq!(unpack_zip(&bytes, LIMITS).unwrap(), files);
    }

    #[test]
    fn names_that_differ_only_by_case_are_duplicates() {
        for (first, second) in [("theme.css", "Theme.css"), ("A/b.css", "a/B.CSS")] {
            let bytes = raw_zip(&[(first, b"1"), (second, b"2")]);
            let result = unpack_zip(&bytes, LIMITS);
            assert!(
                matches!(result, Err(ArchiveError::UnsafePath(_))),
                "{first}: {result:?}"
            );
        }
    }

    #[test]
    fn a_failed_restore_keeps_the_displaced_folder() {
        let parent = tempfile::tempdir().unwrap();
        let old = tempfile::tempdir_in(parent.path()).unwrap();
        let old_path = old.path().to_path_buf();
        fs::create_dir(old_path.join("previous")).unwrap();
        fs::write(old_path.join("previous/a.css"), "1").unwrap();
        // a full folder in the way makes the move back fail
        let target = parent.path().join("night");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("x.css"), "2").unwrap();
        assert!(restore_previous(old, &target).is_err());
        assert!(old_path.join("previous/a.css").exists());
    }

    #[test]
    fn a_vanished_file_is_a_miss() {
        let gone = fs::metadata("definitely/not/here");
        assert!(matches!(missing_as_none(gone), Ok(None)));
        let denied: io::Result<()> = Err(io::ErrorKind::PermissionDenied.into());
        assert!(missing_as_none(denied).is_err());
    }

    #[test]
    fn folders_in_an_archive_are_skipped() {
        let bytes = raw_zip(&[("assets/", b""), ("assets/a.png", b"x")]);
        let files = unpack_zip(&bytes, LIMITS).unwrap();
        assert_eq!(files, [entry("assets/a.png", "x")]);
    }

    #[test]
    fn unsafe_entry_names_are_refused() {
        for name in [
            "../evil.css",
            "a/../../evil.css",
            "/abs.css",
            "C:/evil.css",
            "a\\b.css",
            "a/./b.css",
            "a//b.css",
            "nul.css",
            "AUX",
            "a/con.txt",
            "trailing.",
            "space ",
            "ads.css:stream",
        ] {
            let bytes = raw_zip(&[(name, b"x")]);
            let result = unpack_zip(&bytes, LIMITS);
            assert!(
                matches!(result, Err(ArchiveError::UnsafePath(_))),
                "{name}: {result:?}"
            );
        }
    }

    #[test]
    fn symbolic_links_are_refused() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer
            .add_symlink("link.css", "/etc/passwd", options)
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert!(matches!(
            unpack_zip(&bytes, LIMITS),
            Err(ArchiveError::Link(_))
        ));
    }

    #[test]
    fn limits_hold_even_when_the_archive_lies_about_sizes() {
        let big = vec![b'a'; 257];
        let bytes = raw_zip(&[("big.css", &big)]);
        assert!(matches!(
            unpack_zip(&bytes, LIMITS),
            Err(ArchiveError::FileTooLarge { .. })
        ));
        let many: Vec<(String, Vec<u8>)> =
            (0..9).map(|n| (format!("{n}.css"), vec![b'a'])).collect();
        let items: Vec<(&str, &[u8])> = many
            .iter()
            .map(|(n, d)| (n.as_str(), d.as_slice()))
            .collect();
        assert!(matches!(
            unpack_zip(&raw_zip(&items), LIMITS),
            Err(ArchiveError::TooManyEntries { .. })
        ));
        let spread: Vec<(String, Vec<u8>)> = (0..5)
            .map(|n| (format!("{n}.css"), vec![b'a'; 250]))
            .collect();
        let items: Vec<(&str, &[u8])> = spread
            .iter()
            .map(|(n, d)| (n.as_str(), d.as_slice()))
            .collect();
        assert!(matches!(
            unpack_zip(&raw_zip(&items), LIMITS),
            Err(ArchiveError::TooLarge { .. })
        ));
        assert!(matches!(
            unpack_zip(&vec![0; 2000], LIMITS),
            Err(ArchiveError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_compressed_bomb_is_stopped_while_reading() {
        let bomb = vec![0u8; 100_000];
        let bytes = pack_zip(&[entry("bomb.css", &String::from_utf8(bomb).unwrap())]).unwrap();
        assert!(bytes.len() < 1024);
        assert!(matches!(
            unpack_zip(&bytes, LIMITS),
            Err(ArchiveError::FileTooLarge { .. })
        ));
    }

    #[test]
    fn garbage_is_no_archive() {
        assert!(matches!(
            unpack_zip(b"not a zip", LIMITS),
            Err(ArchiveError::NotArchive(_))
        ));
        assert!(matches!(
            unpack_zip(b"", LIMITS),
            Err(ArchiveError::NotArchive(_))
        ));
    }

    #[test]
    fn replacing_a_folder_swaps_it_whole_and_leaves_no_temporary_folders() {
        let parent = tempfile::tempdir().unwrap();
        replace_dir(
            parent.path(),
            "night",
            &[entry("a.css", "1"), entry("x/b.css", "2")],
        )
        .unwrap();
        replace_dir(parent.path(), "night", &[entry("c.css", "3")]).unwrap();
        let theme = parent.path().join("night");
        assert!(!theme.join("a.css").exists());
        assert_eq!(fs::read_to_string(theme.join("c.css")).unwrap(), "3");
        let names: Vec<String> = fs::read_dir(parent.path())
            .unwrap()
            .map(|child| child.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["night"]);
    }

    #[test]
    fn a_failed_replacement_keeps_the_old_folder() {
        let parent = tempfile::tempdir().unwrap();
        replace_dir(parent.path(), "night", &[entry("a.css", "1")]).unwrap();
        let result = replace_dir(
            parent.path(),
            "night",
            &[entry("b.css", "2"), entry("../escape.css", "3")],
        );
        assert!(matches!(result, Err(ArchiveError::UnsafePath(_))));
        assert_eq!(
            fs::read_to_string(parent.path().join("night/a.css")).unwrap(),
            "1"
        );
        assert!(!parent.path().join("escape.css").exists());
        assert!(matches!(
            replace_dir(parent.path(), "../night", &[]),
            Err(ArchiveError::UnsafePath(_))
        ));
    }

    #[test]
    fn removing_reports_whether_there_was_a_folder() {
        let parent = tempfile::tempdir().unwrap();
        replace_dir(parent.path(), "night", &[entry("a.css", "1")]).unwrap();
        assert!(remove_dir(parent.path(), "night").unwrap());
        assert!(!remove_dir(parent.path(), "night").unwrap());
        assert!(remove_dir(parent.path(), "..").is_err());
    }

    fn segments(path: &str) -> Vec<String> {
        path.split('/').map(str::to_owned).collect()
    }

    #[test]
    fn reading_stays_inside_the_folder() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("night");
        replace_dir(
            parent.path(),
            "night",
            &[entry("theme.css", "css"), entry("assets/a.png", "png")],
        )
        .unwrap();
        fs::write(parent.path().join("secret.txt"), "secret").unwrap();
        let read = |path: &str| read_within(&root, &segments(path), 16).unwrap();
        assert_eq!(read("theme.css"), Some(b"css".to_vec()));
        assert_eq!(read("assets/a.png"), Some(b"png".to_vec()));
        assert_eq!(read("missing.css"), None);
        assert_eq!(read("assets"), None);
        assert_eq!(read("../secret.txt"), None);
        assert_eq!(read("assets/../theme.css"), None);
        assert_eq!(read("a\\b"), None);
        assert_eq!(read("C:"), None);
        assert_eq!(read("theme.css/x"), None);
        let missing_root = parent.path().join("none");
        assert_eq!(
            read_within(&missing_root, &segments("theme.css"), 16).unwrap(),
            None
        );
    }

    #[test]
    fn files_over_the_limit_are_not_served() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("night");
        replace_dir(parent.path(), "night", &[entry("big.css", &"a".repeat(20))]).unwrap();
        assert_eq!(read_within(&root, &segments("big.css"), 16).unwrap(), None);
        assert!(
            read_within(&root, &segments("big.css"), 20)
                .unwrap()
                .is_some()
        );
    }

    #[cfg(windows)]
    fn junction(target: &Path, at: &Path) -> io::Result<()> {
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(at)
            .arg(target)
            .output()?
            .status;
        status
            .success()
            .then_some(())
            .ok_or_else(|| io::Error::other("mklink failed"))
    }

    /// Makes a link to `target`; `false` when the system does not allow it.
    fn link(target: &Path, at: &Path) -> bool {
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, at);
        #[cfg(windows)]
        let made = if target.is_dir() {
            // a junction needs no special right, unlike a symbolic link
            junction(target, at)
        } else {
            std::os::windows::fs::symlink_file(target, at)
        };
        made.is_ok()
    }

    #[test]
    fn a_link_out_of_the_folder_is_not_followed() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("night");
        replace_dir(parent.path(), "night", &[entry("theme.css", "css")]).unwrap();
        let outside = parent.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "secret").unwrap();
        // a folder link always works (junction on Windows), so this cannot pass by skipping
        assert!(link(&outside, &root.join("out")), "no folder link possible");
        assert_eq!(
            read_within(&root, &segments("out/secret.txt"), 16).unwrap(),
            None
        );
        // file links may need rights the machine does not grant
        if link(&outside.join("secret.txt"), &root.join("s.txt")) {
            assert_eq!(read_within(&root, &segments("s.txt"), 16).unwrap(), None);
        }
        assert!(
            read_within(&root, &segments("theme.css"), 16)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn reading_a_folder_refuses_links_and_hidden_files() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("pack");
        fs::create_dir_all(dir.join("assets")).unwrap();
        fs::write(dir.join("manifest.json"), "{}").unwrap();
        fs::write(dir.join("assets/a.png"), "p").unwrap();
        let files = read_tree(&dir, LIMITS).unwrap();
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["assets/a.png", "manifest.json"]);

        fs::write(dir.join(".secret"), "s").unwrap();
        assert!(matches!(
            read_tree(&dir, LIMITS),
            Err(ArchiveError::Hidden(_))
        ));
        fs::remove_file(dir.join(".secret")).unwrap();

        let outside = parent.path().join("outside.txt");
        fs::write(&outside, "x").unwrap();
        if link(&outside, &dir.join("link.css")) {
            assert!(matches!(
                read_tree(&dir, LIMITS),
                Err(ArchiveError::Link(_))
            ));
        }
    }

    #[test]
    fn reading_a_folder_enforces_the_limits() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("pack");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("big.css"), "a".repeat(300)).unwrap();
        assert!(matches!(
            read_tree(&dir, LIMITS),
            Err(ArchiveError::FileTooLarge { .. })
        ));
        fs::remove_file(dir.join("big.css")).unwrap();
        for n in 0..9 {
            fs::write(dir.join(format!("{n}.css")), "a").unwrap();
        }
        assert!(matches!(
            read_tree(&dir, LIMITS),
            Err(ArchiveError::TooManyEntries { .. })
        ));
    }
}
