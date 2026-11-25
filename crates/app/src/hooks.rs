//! Events from Claude Code hooks: kept as an inbox that later features read.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_domain::{HookError, HookEvent, Timestamp};

use crate::sessions::SessionService;
use crate::{AppError, Clock};

#[async_trait]
pub trait HookRepository: Send + Sync {
    /// Keeps the event unless one with its id exists; true when it was new.
    async fn record(&self, event: &HookEvent, received_at: Timestamp) -> Result<bool, AppError>;
    /// Newest first by when they happened.
    async fn recent(&self, limit: u32) -> Result<Vec<HookEvent>, AppError>;
    /// Deletes events received before `before` and returns how many.
    async fn prune_before(&self, before: Timestamp) -> Result<u64, AppError>;
}

/// What the hook hands back to Claude Code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookReply {
    /// Text Claude Code adds to the conversation.
    pub additional_context: Option<String>,
    /// Variables for the environment of the session.
    pub env: BTreeMap<String, String>,
}

#[must_use]
pub fn invalid(error: &HookError) -> AppError {
    let code = match error {
        HookError::InvalidSession => "hook-session-invalid",
        HookError::InvalidCwd => "hook-cwd-invalid",
    };
    AppError::invalid(code, error.to_string())
}

pub struct HookService {
    repository: Arc<dyn HookRepository>,
    clock: Arc<dyn Clock>,
    sessions: Option<Arc<SessionService>>,
}

impl std::fmt::Debug for HookService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookService").finish_non_exhaustive()
    }
}

impl HookService {
    #[must_use]
    pub fn new(repository: Arc<dyn HookRepository>, clock: Arc<dyn Clock>) -> Self {
        Self {
            repository,
            clock,
            sessions: None,
        }
    }

    /// Also keeps the session of each event, in the write that records it.
    #[must_use]
    pub fn with_sessions(mut self, sessions: Arc<SessionService>) -> Self {
        self.sessions = Some(sessions);
        self
    }

    /// Records the event; a resend of the same event changes nothing.
    pub async fn receive(&self, event: &HookEvent) -> Result<HookReply, AppError> {
        let new = match &self.sessions {
            Some(sessions) => sessions.receive(event).await?,
            None => self.repository.record(event, self.clock.now()).await?,
        };
        if new {
            tracing::debug!(
                kind = event.kind().as_str(),
                session = event.session().as_str(),
                "hook event"
            );
        }
        Ok(HookReply::default())
    }

    pub async fn recent(&self, limit: u32) -> Result<Vec<HookEvent>, AppError> {
        self.repository.recent(limit).await
    }

    /// Forgets events older than `keep` and sessions a week after their end.
    pub async fn prune(&self, keep: Duration) -> Result<u64, AppError> {
        let keep = i64::try_from(keep.as_millis()).unwrap_or(i64::MAX);
        let before = Timestamp::from_millis(self.clock.now().as_millis().saturating_sub(keep));
        let pruned = self.repository.prune_before(before).await?;
        if let Some(sessions) = &self.sessions {
            let sessions = sessions.prune().await?;
            tracing::debug!(sessions, "old sessions removed");
        }
        Ok(pruned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_domain_errors_to_stable_codes() {
        assert_eq!(
            invalid(&HookError::InvalidSession).code(),
            "hook-session-invalid"
        );
        assert_eq!(invalid(&HookError::InvalidCwd).code(), "hook-cwd-invalid");
    }
}
