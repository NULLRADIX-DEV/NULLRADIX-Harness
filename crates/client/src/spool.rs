//! Hook events kept on disk while the daemon cannot take them; one file per event.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nullradix_api::{HookEventDto, HookKindDto};
use nullradix_os::write_atomic;
use serde::{Deserialize, Serialize};

const EXTENSION: &str = "json";
/// Events a newer hook wrote that this daemon cannot read yet; retried after an update.
const POSTPONED: &str = "later";
const REJECTED: &str = "bad";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpooledHook {
    pub kind: HookKindDto,
    pub event: HookEventDto,
}

#[derive(Debug, Clone)]
pub struct Spool {
    dir: PathBuf,
}

impl Spool {
    #[must_use]
    pub fn in_dir(data: &Path) -> Self {
        Self {
            dir: data.join("spool").join("hooks"),
        }
    }

    /// Keeps the hook under its event id, so a second push of it changes nothing.
    pub fn push(&self, hook: &SpooledHook) -> io::Result<()> {
        self.create_private_dir()?;
        let json = serde_json::to_vec(hook).map_err(io::Error::other)?;
        let path = self.dir.join(format!("{}.{EXTENSION}", hook.event.id));
        write_atomic(&path, &json)
    }

    /// Prompts may sit here, so only the owner may look inside.
    fn create_private_dir(&self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// Spooled files, oldest first because event ids grow with time.
    pub fn pending(&self) -> io::Result<Vec<PathBuf>> {
        let mut paths = self.files(EXTENSION)?;
        paths.sort();
        Ok(paths)
    }

    /// Unreadable JSON is `InvalidData`; JSON of a newer hook is `Unsupported`.
    pub fn read(path: &Path) -> io::Result<SpooledHook> {
        let bytes = fs::read(path)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        serde_json::from_value(value)
            .map_err(|error| io::Error::new(io::ErrorKind::Unsupported, error))
    }

    /// Keeps a file this daemon cannot read for a later, newer one.
    pub fn postpone(path: &Path) -> io::Result<()> {
        fs::rename(path, path.with_extension(POSTPONED))
    }

    /// Puts postponed files back in line, for a daemon that may read them now.
    pub fn retry_postponed(&self) -> io::Result<usize> {
        let postponed = self.files(POSTPONED)?;
        for path in &postponed {
            fs::rename(path, path.with_extension(EXTENSION))?;
        }
        Ok(postponed.len())
    }

    /// Removes half-written files of hooks that were stopped while spooling.
    pub fn remove_stale_staging(&self, age: Duration) -> io::Result<usize> {
        let mut removed = 0;
        for path in self.files("tmp")? {
            let stale = fs::metadata(&path)?
                .modified()?
                .elapsed()
                .is_ok_and(|elapsed| elapsed > age);
            if stale {
                Self::remove(&path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    fn files(&self, extension: &str) -> io::Result<Vec<PathBuf>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|found| found == extension) {
                paths.push(path);
            }
        }
        Ok(paths)
    }

    pub fn remove(path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Moves an unreadable file aside so it no longer blocks the spool.
    pub fn set_aside(path: &Path) -> io::Result<()> {
        fs::rename(path, path.with_extension(REJECTED))
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn hook(id: u128) -> SpooledHook {
        SpooledHook {
            kind: HookKindDto::Stop,
            event: HookEventDto {
                id: Uuid::from_u128(id),
                session_id: String::from("s-1"),
                cwd: String::from("/repo"),
                occurred_at: 1,
                transcript_path: None,
                prompt: None,
                source: None,
                model: None,
                reason: None,
                message: None,
                notification_type: None,
                agent_id: None,
                agent_type: None,
                ticket: None,
                run: None,
                entrypoint: None,
            },
        }
    }

    #[test]
    fn keeps_hooks_once_and_lists_them_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let spool = Spool::in_dir(dir.path());
        assert_eq!(spool.pending().unwrap(), Vec::<PathBuf>::new());
        spool.push(&hook(2)).unwrap();
        spool.push(&hook(1)).unwrap();
        spool.push(&hook(1)).unwrap();
        let pending = spool.pending().unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(Spool::read(&pending[0]).unwrap(), hook(1));
        Spool::remove(&pending[0]).unwrap();
        Spool::remove(&pending[0]).unwrap();
        assert_eq!(spool.pending().unwrap(), vec![pending[1].clone()]);
    }

    #[test]
    fn hooks_of_a_newer_kind_wait_for_a_daemon_that_knows_them() {
        let dir = tempfile::tempdir().unwrap();
        let spool = Spool::in_dir(dir.path());
        spool.push(&hook(1)).unwrap();
        let path = spool.pending().unwrap().remove(0);
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        json["kind"] = serde_json::json!("future-hook");
        fs::write(&path, json.to_string()).unwrap();
        assert_eq!(
            Spool::read(&path).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        Spool::postpone(&path).unwrap();
        assert_eq!(spool.pending().unwrap(), Vec::<PathBuf>::new());
        assert_eq!(spool.retry_postponed().unwrap(), 1);
        assert_eq!(spool.pending().unwrap(), vec![path]);
    }

    #[test]
    fn stale_staging_files_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let spool = Spool::in_dir(dir.path());
        spool.push(&hook(1)).unwrap();
        let staging = spool.dir.join("x.json.1.tmp");
        fs::write(&staging, b"{").unwrap();
        assert_eq!(
            spool.remove_stale_staging(Duration::from_secs(60)).unwrap(),
            0
        );
        assert_eq!(spool.remove_stale_staging(Duration::ZERO).unwrap(), 1);
        assert!(!staging.exists());
        assert_eq!(spool.pending().unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn only_the_owner_may_read_the_spool() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let spool = Spool::in_dir(dir.path());
        spool.push(&hook(1)).unwrap();
        let mode = fs::metadata(&spool.dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn unreadable_files_are_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let spool = Spool::in_dir(dir.path());
        spool.push(&hook(1)).unwrap();
        let path = spool.pending().unwrap().remove(0);
        fs::write(&path, b"{broken").unwrap();
        let error = Spool::read(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        Spool::set_aside(&path).unwrap();
        assert_eq!(spool.pending().unwrap(), Vec::<PathBuf>::new());
        assert!(path.with_extension("bad").exists());
    }
}
