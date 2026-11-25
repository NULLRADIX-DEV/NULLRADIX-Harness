use std::io;

use super::{SecretBackend, SecretError};
use crate::windows::{CREDENTIAL_MAX, credential_delete, credential_read, credential_write};

const NAME: &str = "Windows Credential Manager";

/// Generic credentials named `<service>/<key>`.
#[derive(Debug)]
pub(super) struct CredentialManager {
    service: String,
}

impl CredentialManager {
    pub(super) fn new(service: &str) -> Self {
        Self {
            service: service.to_owned(),
        }
    }

    fn target(&self, key: &str) -> String {
        format!("{}/{key}", self.service)
    }

    fn failed(source: io::Error) -> SecretError {
        SecretError::Backend {
            backend: NAME,
            source,
        }
    }
}

impl SecretBackend for CredentialManager {
    fn name(&self) -> &'static str {
        NAME
    }

    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        let Some(blob) = credential_read(&self.target(key)).map_err(Self::failed)? else {
            return Ok(None);
        };
        String::from_utf8(blob)
            .map(Some)
            .map_err(|error| Self::failed(io::Error::new(io::ErrorKind::InvalidData, error)))
    }

    fn put(&self, key: &str, value: &str) -> Result<(), SecretError> {
        if value.len() > CREDENTIAL_MAX {
            return Err(Self::failed(io::Error::new(
                io::ErrorKind::InvalidInput,
                "secret too large",
            )));
        }
        credential_write(&self.target(key), &self.service, value.as_bytes()).map_err(Self::failed)
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        credential_delete(&self.target(key)).map_err(Self::failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Needs a Windows session; the Credential Manager asks no one for confirmation.
    #[test]
    fn keeps_replaces_and_deletes_a_credential() {
        let service = format!("nullradix-test-{}", std::process::id());
        let manager = CredentialManager::new(&service);
        let key = "account-token/test";
        assert_eq!(manager.get(key).unwrap(), None);
        manager.put(key, "sk-ant-oat01-fake-one").unwrap();
        assert_eq!(
            manager.get(key).unwrap().as_deref(),
            Some("sk-ant-oat01-fake-one")
        );
        manager.put(key, "sk-ant-oat01-fake-two").unwrap();
        assert_eq!(
            manager.get(key).unwrap().as_deref(),
            Some("sk-ant-oat01-fake-two")
        );
        manager.delete(key).unwrap();
        manager.delete(key).unwrap();
        assert_eq!(manager.get(key).unwrap(), None);
    }

    #[test]
    fn refuses_secrets_the_credential_manager_cannot_hold() {
        let manager = CredentialManager::new("nullradix-test-large");
        let large = "x".repeat(CREDENTIAL_MAX + 1);
        assert!(manager.put("large", &large).is_err());
        assert_eq!(manager.get("large").unwrap(), None);
    }
}
