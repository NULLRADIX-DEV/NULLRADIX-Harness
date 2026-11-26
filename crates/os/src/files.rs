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
    // create_new refuses a pre-created file or link instead of reusing it
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    if private {
        use std::os::windows::fs::OpenOptionsExt;
        // no sharing, so nobody reads the file before its access list is set
        options
            .share_mode(0)
            .access_mode(crate::windows::RESTRICT_ACCESS);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = private;
    options.open(path)
}

/// Fills the staging file and closes it; the handle is held until the access list is set.
fn fill(mut file: File, bytes: &[u8], private: bool) -> io::Result<()> {
    #[cfg(windows)]
    if private {
        crate::windows::restrict_handle_to_current_user(&file)?;
    }
    #[cfg(not(windows))]
    let _ = private;
    file.write_all(bytes)?;
    file.sync_all()
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
    let file = create(&staging, private)?;
    if let Err(error) = fill(file, bytes, private) {
        let _ = fs::remove_file(&staging);
        return Err(error);
    }
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

    #[cfg(windows)]
    #[test]
    fn a_private_file_has_an_owner_only_protected_acl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli-token");
        write_private(&path, b"one").unwrap();
        write_private(&path, b"two").unwrap();
        let user = crate::windows::UserSid::current().unwrap();
        let acl = crate::windows::acl_summary(&path).unwrap();
        // owned by the user, protected from inheritance, one entry: full access for that SID
        assert_eq!(acl.owner, user.text());
        assert!(acl.protected);
        assert_eq!(acl.entries, 1);
        assert_eq!(acl.allowed, [(user.text().to_owned(), 0x001F_01FF)]);
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
    }

    #[cfg(unix)]
    #[test]
    fn a_private_file_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli-token");
        write_private(&path, b"one").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_pre_existing_staging_path_fails_the_write_and_stays_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli-token");
        let staging = path.with_extension(format!("{}.tmp", std::process::id()));
        // a folder cannot be removed as a leftover file, so create_new meets it
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("keep"), b"theirs").unwrap();
        assert!(write_private(&path, b"secret").is_err());
        assert!(write_atomic(&path, b"secret").is_err());
        assert!(!path.exists());
        assert_eq!(fs::read(staging.join("keep")).unwrap(), b"theirs");
        assert_eq!(fs::read_dir(&staging).unwrap().count(), 1);
    }

    #[test]
    fn a_failed_write_leaves_no_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("target");
        // a folder at the target makes the rename fail after the staging file was written
        fs::create_dir(&path).unwrap();
        assert!(write_private(&path, b"secret").is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn treats_sharing_violations_as_transient() {
        assert!(transient(&io::Error::from_raw_os_error(32)));
        assert!(!transient(&io::Error::from(io::ErrorKind::NotFound)));
    }
}
