use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

const RENAME_ATTEMPTS: u32 = 10;
const RENAME_PAUSE: Duration = Duration::from_millis(50);

static SERIAL: AtomicU64 = AtomicU64::new(0);

/// Replaces `path` with `bytes` in one rename, so readers never see a partial file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace(path, bytes, false)
}

/// Like [`write_atomic`], and the file is readable by its owner only from the start.
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
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let me = nix::unistd::Uid::effective().as_raw();
        tighten_dir(dir, me, me)
    }
    #[cfg(windows)]
    {
        fs::create_dir_all(dir)?;
        crate::windows::restrict_to_current_user(dir)
    }
}

/// Sets mode 0700 on the folder behind `dir`, also through a link of its owner.
#[cfg(unix)]
fn tighten_dir(dir: &Path, link_owner: u32, folder_owner: u32) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let link = fs::symlink_metadata(dir)?;
    let linked = link.file_type().is_symlink();
    if linked {
        owned_by(dir, "the link", link.uid(), link_owner)?;
    }
    let target = if linked {
        fs::canonicalize(dir)?
    } else {
        dir.to_path_buf()
    };
    // no link is followed on the way in, so the mode goes to the folder that was checked
    let flags = nix::fcntl::OFlag::O_DIRECTORY | nix::fcntl::OFlag::O_NOFOLLOW;
    let folder = OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits())
        .open(&target)?;
    owned_by(dir, "the folder", folder.metadata()?.uid(), folder_owner)?;
    folder.set_permissions(fs::Permissions::from_mode(0o700))
}

/// Refuses a link or a folder of another user.
#[cfg(unix)]
fn owned_by(dir: &Path, what: &str, owner: u32, me: u32) -> io::Result<()> {
    if owner == me {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "cannot make {} private, another user owns {what}",
            dir.display()
        ),
    ))
}

/// Creates `dir` private to its owner on Unix and plain on Windows.
pub fn create_data_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        create_private_dir(dir)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Creates `path` when missing and makes it readable by its owner only: mode 0600 on Unix.
pub fn ensure_private_file(path: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    // no truncation, an existing file keeps its content
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?;
    tighten_private_file(path)
}

/// Makes an existing file private to its owner on Unix, a missing file is fine.
pub fn tighten_private_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// A staging name of its own per call, so writers of one path never meet.
fn staging_for(path: &Path) -> PathBuf {
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!("{}-{serial}.tmp", std::process::id()))
}

fn replace(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    replace_via(path, &staging_for(path), bytes, private)
}

fn replace_via(path: &Path, staging: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    // a leftover of a dead process with the same pid would block create_new for good
    let _ = fs::remove_file(staging);
    let file = create(staging, private)?;
    if let Err(error) = fill(file, bytes, private) {
        let _ = fs::remove_file(staging);
        return Err(error);
    }
    let mut attempt = 1;
    loop {
        match fs::rename(staging, path) {
            Ok(()) => return Ok(()),
            Err(error) if attempt < RENAME_ATTEMPTS && transient(&error) => {
                attempt += 1;
                thread::sleep(RENAME_PAUSE);
            }
            Err(error) => {
                let _ = fs::remove_file(staging);
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

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn a_data_dir_is_private_and_tightens_a_folder_of_an_older_install() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("data");
        fs::create_dir(&old).unwrap();
        fs::set_permissions(&old, fs::Permissions::from_mode(0o755)).unwrap();
        create_data_dir(&old).unwrap();
        assert_eq!(mode_of(&old), 0o700);
        let nested = old.join("runs").join("one");
        create_data_dir(&nested).unwrap();
        assert_eq!(mode_of(&old.join("runs")), 0o700);
        assert_eq!(mode_of(&nested), 0o700);
    }

    #[cfg(unix)]
    fn linked_folder(dir: &Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let target = dir.join("elsewhere");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let link = dir.join("data");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        (link, target)
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_folder_of_the_owner_tightens_that_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (link, target) = linked_folder(dir.path());
        create_private_dir(&link).unwrap();
        assert_eq!(mode_of(&target), 0o700);
        create_data_dir(&link).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let nested = link.join("runs");
        create_data_dir(&nested).unwrap();
        assert_eq!(mode_of(&target.join("runs")), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_or_target_of_another_user_is_refused_and_keeps_its_mode() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let (link, target) = linked_folder(dir.path());
        let mine = fs::symlink_metadata(&link).unwrap().uid();
        let error = tighten_dir(&link, mine + 1, mine).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("cannot make"), "{error}");
        assert!(error.to_string().contains("owns the link"), "{error}");
        let error = tighten_dir(&link, mine, mine + 1).unwrap_err();
        assert!(error.to_string().contains("owns the folder"), "{error}");
        let error = tighten_dir(&target, mine, mine + 1).unwrap_err();
        assert!(error.to_string().contains("owns the folder"), "{error}");
        assert_eq!(mode_of(&target), 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, b"x").unwrap();
        let link = dir.path().join("data");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(create_private_dir(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_private_file_is_created_or_tightened_without_losing_its_content() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nullradix.db");
        ensure_private_file(&path).unwrap();
        assert_eq!(mode_of(&path), 0o600);
        fs::write(&path, b"rows").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        ensure_private_file(&path).unwrap();
        assert_eq!(mode_of(&path), 0o600);
        assert_eq!(fs::read(&path).unwrap(), b"rows");
    }

    #[cfg(unix)]
    #[test]
    fn tightening_leaves_a_missing_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nullradix.db-wal");
        tighten_private_file(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn a_pre_existing_staging_path_fails_the_write_and_stays_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli-token");
        let staging = path.with_extension("planted.tmp");
        // a folder cannot be removed as a leftover file, so create_new meets it
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("keep"), b"theirs").unwrap();
        assert!(replace_via(&path, &staging, b"secret", true).is_err());
        assert!(replace_via(&path, &staging, b"secret", false).is_err());
        assert!(!path.exists());
        assert_eq!(fs::read(staging.join("keep")).unwrap(), b"theirs");
        assert_eq!(fs::read_dir(&staging).unwrap().count(), 1);
    }

    #[test]
    fn a_leftover_staging_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel");
        let staging = path.with_extension("left.tmp");
        fs::write(&staging, b"old").unwrap();
        replace_via(&path, &staging, b"new", false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert!(!staging.exists());
    }

    #[test]
    fn writers_of_one_path_do_not_meet() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel");
        let writers: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                thread::spawn(move || {
                    for _ in 0..25 {
                        write_atomic(&path, b"stop").unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        assert_eq!(fs::read(&path).unwrap(), b"stop");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
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
