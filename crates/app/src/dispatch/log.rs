//! The dispatcher's own log: what it decided and why, kept in the database.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{Id, Run, Timestamp};
use strum::{EnumString, IntoStaticStr};
use uuid::Uuid;

use crate::{AppError, Clock, IdGenerator};

/// How long a line stays in the log.
pub const LOG_KEPT: std::time::Duration = std::time::Duration::from_hours(30 * 24);

/// Longest log message kept, in characters.
pub const MESSAGE_MAX: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub id: Uuid,
    pub at: Timestamp,
    pub level: LogLevel,
    pub message: String,
    pub run: Option<Id<Run>>,
    /// Ticket or question key the line is about.
    pub subject: Option<String>,
}

#[async_trait]
pub trait DispatcherLogRepository: Send + Sync {
    async fn append(&self, line: &LogLine) -> Result<(), AppError>;
    /// Newest first.
    async fn recent(&self, limit: u32) -> Result<Vec<LogLine>, AppError>;
    /// Removes the lines older than `before`.
    async fn prune_before(&self, before: Timestamp) -> Result<u64, AppError>;
}

/// Writes lines; a line that cannot be written is reported to the process log and dropped.
pub struct DispatchLog {
    repository: Arc<dyn DispatcherLogRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl std::fmt::Debug for DispatchLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchLog").finish_non_exhaustive()
    }
}

impl DispatchLog {
    #[must_use]
    pub fn new(
        repository: Arc<dyn DispatcherLogRepository>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            repository,
            clock,
            ids,
        }
    }

    pub async fn write(
        &self,
        level: LogLevel,
        message: &str,
        subject: Option<&str>,
        run: Option<Id<Run>>,
    ) {
        let line = LogLine {
            id: self.ids.next_uuid(),
            at: self.clock.now(),
            level,
            message: message.chars().take(MESSAGE_MAX).collect(),
            run,
            subject: subject.map(str::to_owned),
        };
        if let Err(error) = self.repository.append(&line).await {
            tracing::warn!(%error, "cannot write the dispatcher log");
        }
    }

    pub async fn info(&self, message: &str, subject: Option<&str>) {
        self.write(LogLevel::Info, message, subject, None).await;
    }

    pub async fn error(&self, message: &str, subject: Option<&str>) {
        self.write(LogLevel::Error, message, subject, None).await;
    }

    pub async fn recent(&self, limit: u32) -> Result<Vec<LogLine>, AppError> {
        self.repository.recent(limit).await
    }

    /// Drops the lines older than `LOG_KEPT`.
    pub async fn prune(&self) {
        let kept = i64::try_from(LOG_KEPT.as_millis()).unwrap_or(i64::MAX);
        let before = Timestamp::from_millis(self.clock.now().as_millis().saturating_sub(kept));
        if let Err(error) = self.repository.prune_before(before).await {
            tracing::warn!(%error, "cannot prune the dispatcher log");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_use_their_stored_names() {
        for level in [
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ] {
            assert_eq!(level.as_str().parse::<LogLevel>(), Ok(level));
        }
    }
}
