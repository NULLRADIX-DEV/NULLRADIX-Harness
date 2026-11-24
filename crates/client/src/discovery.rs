use std::fs::{self, File};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Written by the running daemon so clients can find it without the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub pid: u32,
    pub address: SocketAddr,
    pub version: String,
    pub api_version: u32,
    pub started_at: i64,
}

impl Discovery {
    /// False when the recorded process is gone, which makes the file stale without a network call.
    #[must_use]
    pub fn process_alive(&self) -> bool {
        nullradix_os::process_alive(self.pid)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("cannot access {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid discovery file {path}")]
    Invalid {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone)]
pub struct DiscoveryFile {
    path: PathBuf,
}

impl DiscoveryFile {
    #[must_use]
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            path: dir.join("daemon.json"),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> Result<Option<Discovery>, DiscoveryError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(self.io(source)),
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|source| DiscoveryError::Invalid {
                path: self.path.clone(),
                source,
            })
    }

    /// Replaces the file in one rename so readers never see a partial write.
    pub fn write(&self, discovery: &Discovery) -> Result<(), DiscoveryError> {
        let json =
            serde_json::to_vec_pretty(discovery).map_err(|source| DiscoveryError::Invalid {
                path: self.path.clone(),
                source,
            })?;
        let staging = self.path.with_extension("json.tmp");
        let mut file = File::create(&staging).map_err(|source| self.io(source))?;
        file.write_all(&json)
            .and_then(|()| file.sync_all())
            .map_err(|source| self.io(source))?;
        drop(file);
        fs::rename(&staging, &self.path).map_err(|source| self.io(source))
    }

    pub fn remove(&self) -> Result<(), DiscoveryError> {
        match fs::remove_file(&self.path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(self.io(error)),
            _ => Ok(()),
        }
    }

    fn io(&self, source: io::Error) -> DiscoveryError {
        DiscoveryError::Io {
            path: self.path.clone(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Discovery {
        Discovery {
            pid: 42,
            address: "127.0.0.1:4711".parse().unwrap(),
            version: String::from("0.1.0"),
            api_version: 1,
            started_at: 1_000,
        }
    }

    #[test]
    fn round_trips_and_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let file = DiscoveryFile::in_dir(dir.path());
        assert_eq!(file.read().unwrap(), None);
        file.write(&sample()).unwrap();
        let mut newer = sample();
        newer.pid = 43;
        file.write(&newer).unwrap();
        assert_eq!(file.read().unwrap(), Some(newer));
    }

    #[test]
    fn remove_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let file = DiscoveryFile::in_dir(dir.path());
        file.write(&sample()).unwrap();
        file.remove().unwrap();
        file.remove().unwrap();
        assert_eq!(file.read().unwrap(), None);
    }

    #[test]
    fn rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let file = DiscoveryFile::in_dir(dir.path());
        fs::write(file.path(), "{ nope").unwrap();
        assert!(matches!(file.read(), Err(DiscoveryError::Invalid { .. })));
    }
}
