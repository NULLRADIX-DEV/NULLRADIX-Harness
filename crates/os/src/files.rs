use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

const RENAME_ATTEMPTS: u32 = 10;
const RENAME_PAUSE: Duration = Duration::from_millis(50);

/// Replaces `path` with `bytes` in one rename, so readers never see a partial file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staging = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = File::create(&staging)?;
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
