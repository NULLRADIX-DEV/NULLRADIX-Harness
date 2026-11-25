use std::io;

use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

use super::{SecretBackend, SecretError};

const NAME: &str = "macOS Keychain";
/// `errSecItemNotFound`
const NOT_FOUND: i32 = -25_300;

/// Generic passwords of the login keychain, one per key under the service name.
#[derive(Debug)]
pub(super) struct Keychain {
    service: String,
}

impl Keychain {
    pub(super) fn new(service: &str) -> Self {
        Self {
            service: service.to_owned(),
        }
    }

    fn failed(error: security_framework::base::Error) -> SecretError {
        SecretError::Backend {
            backend: NAME,
            source: io::Error::other(error.to_string()),
        }
    }
}

impl SecretBackend for Keychain {
    fn name(&self) -> &'static str {
        NAME
    }

    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        match get_generic_password(&self.service, key) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| SecretError::Backend {
                    backend: NAME,
                    source: io::Error::new(io::ErrorKind::InvalidData, error),
                }),
            Err(error) if error.code() == NOT_FOUND => Ok(None),
            Err(error) => Err(Self::failed(error)),
        }
    }

    fn put(&self, key: &str, value: &str) -> Result<(), SecretError> {
        set_generic_password(&self.service, key, value.as_bytes()).map_err(Self::failed)
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        match delete_generic_password(&self.service, key) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == NOT_FOUND => Ok(()),
            Err(error) => Err(Self::failed(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "the keychain may ask for confirmation, run it by hand on a Mac"]
    fn keeps_replaces_and_deletes_a_password() {
        let keychain = Keychain::new(&format!("nullradix-test-{}", std::process::id()));
        let key = "account-token/test";
        assert_eq!(keychain.get(key).unwrap(), None);
        keychain.put(key, "sk-ant-oat01-fake-one").unwrap();
        keychain.put(key, "sk-ant-oat01-fake-two").unwrap();
        assert_eq!(
            keychain.get(key).unwrap().as_deref(),
            Some("sk-ant-oat01-fake-two")
        );
        keychain.delete(key).unwrap();
        keychain.delete(key).unwrap();
        assert_eq!(keychain.get(key).unwrap(), None);
    }
}
