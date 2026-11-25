//! Where NULLRADIX keeps its own secrets; the database only stores references.

use std::collections::HashMap;
use std::fmt;

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::AppError;

/// A stored secret; it never shows in debug output.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(..)")
    }
}

/// Failure of the system's secret storage; adapters map their errors to this.
#[must_use]
pub fn secret_store_failed(source: impl std::error::Error + Send + Sync + 'static) -> AppError {
    AppError::unavailable("secret-store-failed", "the secret store did not answer")
        .with_source(source)
}

/// Keeps secrets by key in the Credential Manager, the Keychain, the Secret Service or a file.
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<SecretValue>, AppError>;
    /// Replaces the secret under `key`.
    async fn put(&self, key: &str, value: &SecretValue) -> Result<(), AppError>;
    /// Removes the secret; a missing one is no error.
    async fn delete(&self, key: &str) -> Result<(), AppError>;
}

/// Secret store in memory, for tests.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    entries: Mutex<HashMap<String, String>>,
}

impl MemorySecretStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keys with a secret, sorted.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.entries.lock().keys().cloned().collect();
        keys.sort();
        keys
    }
}

#[async_trait]
impl SecretStore for MemorySecretStore {
    async fn get(&self, key: &str) -> Result<Option<SecretValue>, AppError> {
        Ok(self.entries.lock().get(key).cloned().map(SecretValue))
    }

    async fn put(&self, key: &str, value: &SecretValue) -> Result<(), AppError> {
        self.entries
            .lock()
            .insert(key.to_owned(), value.expose().to_owned());
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), AppError> {
        self.entries.lock().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_show_in_debug_output() {
        let value = SecretValue::new("sk-ant-oat01-secret");
        assert_eq!(format!("{value:?}"), "SecretValue(..)");
    }

    #[tokio::test]
    async fn the_memory_store_keeps_replaces_and_deletes() {
        let store = MemorySecretStore::new();
        assert_eq!(store.get("a").await.unwrap(), None);
        store.put("a", &SecretValue::new("one")).await.unwrap();
        store.put("a", &SecretValue::new("two")).await.unwrap();
        assert_eq!(store.get("a").await.unwrap().unwrap().expose(), "two");
        assert_eq!(store.keys(), vec![String::from("a")]);
        store.delete("a").await.unwrap();
        store.delete("a").await.unwrap();
        assert_eq!(store.get("a").await.unwrap(), None);
    }

    #[test]
    fn store_failures_hide_the_cause_from_clients() {
        let error = secret_store_failed(std::io::Error::other("locked"));
        assert_eq!(error.code(), "secret-store-failed");
        assert!(!error.message().contains("locked"));
    }
}
