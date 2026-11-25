use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

const RENAME_ATTEMPTS: u32 = 10;
const RENAME_PAUSE: Duration = Duration::from_millis(50);

/// Replaces `path` with `bytes` in one rename, so readers never see a partial file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace(path, bytes, false)
}

/// Like [`write_atomic`], and on Unix the file is readable by its owner only from the start.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace(path, bytes, true)
}

fn create(path: &Path, private: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let file = options.open(path)?;
    #[cfg(windows)]
    if private {
        crate::windows::restrict_to_current_user(path)?;
    }
    Ok(file)
}

/// Creates `dir` for its owner only: mode 0700, or an owner-only access list on Windows.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        fs::create_dir_all(dir)?;
        crate::windows::restrict_to_current_user(dir)
    }
}

fn replace(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let staging = path.with_extension(format!("{}.tmp", std::process::id()));
    if private {
        // a leftover from a dead process may carry looser permissions
        let _ = fs::remove_file(&staging);
    }
    let mut file = create(&staging, private)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    let mut attempt = 1;
    loop {
        match fs::rename(&staging, path) {
            Ok(()) => return Ok(()),
            Err(error) if attempt < RENAME_ATTEMPTS && transient(&error) => {
                attempt += 1;
                thread::sleep(RENAME_PAUSE);
            }
            Err(error) => {
                let _ = fs::remove_file(&staging);
                return Err(error);
            }
        }
    }
}

/// Virus scanners and indexers briefly lock fresh files on Windows.
fn transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::ResourceBusy
    ) || matches!(error.raw_os_error(), Some(32 | 33))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_replaces_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn treats_sharing_violations_as_transient() {
        assert!(transient(&io::Error::from_raw_os_error(32)));
        assert!(!transient(&io::Error::from(io::ErrorKind::NotFound)));
    }
}
