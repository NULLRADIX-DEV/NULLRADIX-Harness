//! Hook events kept on disk while the daemon cannot take them; one file per event.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use nullradix_api::{HookEventDto, HookKindDto};
use nullradix_os::write_atomic;
use serde::{Deserialize, Serialize};

const EXTENSION: &str = "json";

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
        fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_vec(hook).map_err(io::Error::other)?;
        let path = self.dir.join(format!("{}.{EXTENSION}", hook.event.id));
        write_atomic(&path, &json)
    }

    /// Spooled files, oldest first because event ids grow with time.
    pub fn pending(&self) -> io::Result<Vec<PathBuf>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == EXTENSION)
            {
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
    }

    pub fn read(path: &Path) -> io::Result<SpooledHook> {
        let bytes = fs::read(path)?;
        serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn remove(path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Moves an unreadable file aside so it no longer blocks the spool.
    pub fn set_aside(path: &Path) -> io::Result<()> {
        fs::rename(path, path.with_extension("bad"))
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
