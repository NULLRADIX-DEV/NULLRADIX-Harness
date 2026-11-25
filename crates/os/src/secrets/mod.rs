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

/// The system store for `service`; a failing `secret-tool` is an error, never a plaintext fallback.
pub fn system_secrets(
    service: &str,
    fallback_dir: &Path,
) -> Result<Box<dyn SecretBackend>, SecretError> {
    #[cfg(windows)]
    {
        let _ = fallback_dir;
        Ok(Box::new(credential_manager::CredentialManager::new(
            service,
        )))
    }
    #[cfg(target_os = "macos")]
    {
        let _ = fallback_dir;
        Ok(Box::new(keychain::Keychain::new(service)))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        choose(secret_tool::SecretTool::detect(service), fallback_dir)
    }
}

/// Linux uses the private file in `fallback_dir` only when `secret-tool` is not installed.
#[cfg_attr(
    any(windows, target_os = "macos"),
    allow(dead_code, reason = "only Linux and other Unix systems choose")
)]
fn choose(
    detection: secret_tool::Detection,
    fallback_dir: &Path,
) -> Result<Box<dyn SecretBackend>, SecretError> {
    match detection {
        secret_tool::Detection::Ready(tool) => Ok(Box::new(tool)),
        secret_tool::Detection::NotInstalled => Ok(Box::new(FileSecrets::in_dir(fallback_dir))),
        secret_tool::Detection::Failing(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failing() -> SecretError {
        SecretError::Backend {
            backend: "Secret Service",
            source: io::Error::other("no session"),
        }
    }

    #[test]
    fn a_missing_secret_tool_means_the_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let chosen = choose(secret_tool::Detection::NotInstalled, dir.path()).unwrap();
        assert_eq!(chosen.name(), "private file");
    }

    #[test]
    fn a_working_secret_service_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let tool = secret_tool::SecretTool::with_runner(
            "nullradix",
            Box::new(|_, _| {
                Ok(crate::command::Reply {
                    success: false,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }),
        );
        let detection = secret_tool::Detection::of(tool);
        assert_eq!(
            choose(detection, dir.path()).unwrap().name(),
            "Secret Service"
        );
    }

    #[test]
    fn a_secret_service_that_fails_is_an_error_and_writes_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let error = choose(secret_tool::Detection::Failing(failing()), dir.path());
        assert!(error.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
