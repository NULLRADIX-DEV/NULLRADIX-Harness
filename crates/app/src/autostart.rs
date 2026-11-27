//! Whether the app starts when the person logs in; the system entry itself is the stored switch.

use std::sync::Arc;

use async_trait::async_trait;

use crate::{AppError, ErrorKind};

/// The login entry of this host.
#[async_trait]
pub trait Autostart: Send + Sync {
    /// `false` on hosts that cannot start programs at login, such as a container.
    fn available(&self) -> bool;
    async fn is_enabled(&self) -> Result<bool, String>;
    /// Creates or refreshes the entry, or removes it.
    async fn set(&self, enabled: bool) -> Result<(), String>;
    /// Points an existing entry at the running program when that is right; keeps its switch.
    async fn refresh(&self) -> Result<(), String>;
}

/// For hosts without a login: nothing starts there.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAutostart;

#[async_trait]
impl Autostart for NoAutostart {
    fn available(&self) -> bool {
        false
    }

    async fn is_enabled(&self) -> Result<bool, String> {
        Ok(false)
    }

    async fn set(&self, _enabled: bool) -> Result<(), String> {
        Err(String::from("this host has no login to start at"))
    }

    async fn refresh(&self) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutostartStatus {
    pub available: bool,
    pub enabled: bool,
}

pub struct AutostartService {
    port: Arc<dyn Autostart>,
}

impl std::fmt::Debug for AutostartService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutostartService").finish_non_exhaustive()
    }
}

impl AutostartService {
    #[must_use]
    pub fn new(port: Arc<dyn Autostart>) -> Self {
        Self { port }
    }

    pub async fn status(&self) -> Result<AutostartStatus, AppError> {
        if !self.port.available() {
            return Ok(AutostartStatus {
                available: false,
                enabled: false,
            });
        }
        let enabled = self
            .port
            .is_enabled()
            .await
            .map_err(|message| failed(&message))?;
        Ok(AutostartStatus {
            available: true,
            enabled,
        })
    }

    pub async fn set(&self, enabled: bool) -> Result<AutostartStatus, AppError> {
        if !self.port.available() {
            return Err(AppError::unavailable(
                "autostart-unavailable",
                "this host cannot start NULLRADIX at login",
            ));
        }
        self.port
            .set(enabled)
            .await
            .map_err(|message| failed(&message))?;
        self.status().await
    }

    /// Keeps an existing entry pointing at a program that exists; a failure only goes to the log.
    pub async fn refresh(&self) {
        if self.port.available()
            && let Err(error) = self.port.refresh().await
        {
            tracing::warn!(%error, "cannot refresh the login entry");
        }
    }
}

fn failed(message: &str) -> AppError {
    AppError::new(
        ErrorKind::Internal,
        "autostart-failed",
        format!("cannot change the login entry: {message}"),
    )
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use super::*;

    #[derive(Default)]
    struct Fake {
        state: Mutex<Option<bool>>,
        writes: Mutex<Vec<bool>>,
        refreshes: Mutex<usize>,
        fail: bool,
    }

    #[async_trait]
    impl Autostart for Fake {
        fn available(&self) -> bool {
            true
        }

        async fn is_enabled(&self) -> Result<bool, String> {
            Ok(self.state.lock().unwrap_or(false))
        }

        async fn set(&self, enabled: bool) -> Result<(), String> {
            if self.fail {
                return Err(String::from("denied"));
            }
            *self.state.lock() = Some(enabled);
            self.writes.lock().push(enabled);
            Ok(())
        }

        async fn refresh(&self) -> Result<(), String> {
            *self.refreshes.lock() += 1;
            Ok(())
        }
    }

    #[tokio::test]
    async fn the_switch_follows_the_entry() {
        let service = AutostartService::new(Arc::new(Fake::default()));
        assert!(!service.status().await.unwrap().enabled);
        assert!(service.set(true).await.unwrap().enabled);
        assert!(service.status().await.unwrap().enabled);
        assert!(!service.set(false).await.unwrap().enabled);
    }

    #[tokio::test]
    async fn a_host_without_login_refuses_the_switch() {
        let service = AutostartService::new(Arc::new(NoAutostart));
        let status = service.status().await.unwrap();
        assert_eq!((status.available, status.enabled), (false, false));
        let error = service.set(true).await.unwrap_err();
        assert_eq!(error.code(), "autostart-unavailable");
        assert_eq!(error.kind(), ErrorKind::Unavailable);
    }

    #[tokio::test]
    async fn a_failing_system_is_reported_with_its_reason() {
        let service = AutostartService::new(Arc::new(Fake {
            fail: true,
            ..Fake::default()
        }));
        let error = service.set(true).await.unwrap_err();
        assert_eq!(error.code(), "autostart-failed");
        assert!(error.message().contains("denied"), "{}", error.message());
    }

    #[tokio::test]
    async fn refresh_asks_the_port_and_never_flips_the_switch() {
        let fake = Arc::new(Fake::default());
        let service = AutostartService::new(fake.clone());
        service.refresh().await;
        assert_eq!(*fake.refreshes.lock(), 1);
        assert!(fake.writes.lock().is_empty());
        AutostartService::new(Arc::new(NoAutostart)).refresh().await;
    }
}
