//! Secret storage: Windows Credential Manager, macOS Keychain, Secret Service or a private file.

#[cfg(windows)]
mod credential_manager;
mod file;
#[cfg(target_os = "macos")]
mod keychain;
mod secret_tool;

use std::fmt::Debug;
use std::io;
use std::path::{Path, PathBuf};

pub use file::FileSecrets;

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("{backend} did not accept the request")]
    Backend {
        backend: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("the secret file {} is damaged", path.display())]
    Damaged { path: PathBuf },
}

/// One place that keeps secrets by key; calls block, so async callers move them to a thread.
pub trait SecretBackend: Send + Sync + Debug {
    fn name(&self) -> &'static str;
    fn get(&self, key: &str) -> Result<Option<String>, SecretError>;
    /// Replaces the secret under `key`.
    fn put(&self, key: &str, value: &str) -> Result<(), SecretError>;
    /// Removes the secret; a missing one is no error.
    fn delete(&self, key: &str) -> Result<(), SecretError>;
}

/// The system store for `service`; without a Secret Service, Linux keeps a file in `fallback_dir`.
#[must_use]
pub fn system_secrets(service: &str, fallback_dir: &Path) -> Box<dyn SecretBackend> {
    #[cfg(windows)]
    {
        let _ = fallback_dir;
        Box::new(credential_manager::CredentialManager::new(service))
    }
    #[cfg(target_os = "macos")]
    {
        let _ = fallback_dir;
        Box::new(keychain::Keychain::new(service))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        match secret_tool::SecretTool::detect(service) {
            Some(tool) => Box::new(tool),
            None => Box::new(FileSecrets::in_dir(fallback_dir)),
        }
    }
}
