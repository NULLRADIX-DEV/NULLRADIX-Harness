//! The daemon's secret store: the platform backend behind the app's `SecretStore` port.

use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::secrets::{SecretStore, SecretValue, secret_store_failed};
use nullradix_os::{FileSecrets, SecretBackend, system_secrets};

use crate::config::Config;

pub(crate) struct OsSecretStore {
    backend: Arc<dyn SecretBackend>,
}

impl OsSecretStore {
    pub(crate) fn new(backend: Box<dyn SecretBackend>) -> Self {
        Self {
            backend: Arc::from(backend),
        }
    }

    /// The system store, or a private file under a home override; detection runs off the runtime.
    pub(crate) async fn open(config: &Config) -> anyhow::Result<Self> {
        let (isolated, service, data) = (
            config.isolated,
            config.profile.app_name(),
            config.paths.data.clone(),
        );
        let backend = tokio::task::spawn_blocking(move || -> anyhow::Result<Box<dyn SecretBackend>> {
            if isolated {
                return Ok(Box::new(FileSecrets::in_dir(&data)));
            }
            system_secrets(service, &data).context(
                "the system secret store cannot be used; unlock it or start its session, or remove secret-tool to keep secrets in a private file",
            )
        })
        .await
        .context("the secret store check did not finish")??;
        tracing::info!(backend = backend.name(), "secret store ready");
        Ok(Self::new(backend))
    }

    async fn blocking<T, F>(&self, call: F) -> Result<T, AppError>
    where
        T: Send + 'static,
        F: FnOnce(&dyn SecretBackend) -> Result<T, nullradix_os::SecretError> + Send + 'static,
    {
        let backend = Arc::clone(&self.backend);
        tokio::task::spawn_blocking(move || call(backend.as_ref()))
            .await
            .map_err(secret_store_failed)?
            .map_err(secret_store_failed)
    }
}

#[async_trait]
impl SecretStore for OsSecretStore {
    async fn get(&self, key: &str) -> Result<Option<SecretValue>, AppError> {
        let key = key.to_owned();
        let found = self.blocking(move |backend| backend.get(&key)).await?;
        Ok(found.map(SecretValue::new))
    }

    async fn put(&self, key: &str, value: &SecretValue) -> Result<(), AppError> {
        let (key, value) = (key.to_owned(), value.expose().to_owned());
        self.blocking(move |backend| backend.put(&key, &value))
            .await
    }

    async fn delete(&self, key: &str) -> Result<(), AppError> {
        let key = key.to_owned();
        self.blocking(move |backend| backend.delete(&key)).await
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn file_in(dir: &Path) -> OsSecretStore {
        OsSecretStore::new(Box::new(FileSecrets::in_dir(dir)))
    }

    #[tokio::test]
    async fn stores_through_the_backend_on_blocking_threads() {
        let dir = tempfile::tempdir().unwrap();
        let store = file_in(dir.path());
        assert_eq!(store.get("a").await.unwrap(), None);
        store
            .put("a", &SecretValue::new("sk-ant-oat01-fake"))
            .await
            .unwrap();
        assert_eq!(
            store.get("a").await.unwrap().unwrap().expose(),
            "sk-ant-oat01-fake"
        );
        store.delete("a").await.unwrap();
        assert_eq!(store.get("a").await.unwrap(), None);
    }

    #[tokio::test]
    async fn backend_failures_become_a_stable_error_without_details() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("secrets")).unwrap();
        std::fs::write(dir.path().join("secrets").join("secrets.json"), "{ damaged").unwrap();
        let store = file_in(dir.path());
        let error = store.get("a").await.unwrap_err();
        assert_eq!(error.code(), "secret-store-failed");
        assert_eq!(error.kind(), nullradix_app::ErrorKind::Unavailable);
    }
}
