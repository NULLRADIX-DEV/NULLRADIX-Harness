//! How often research and planning ran for a question or ticket, and the automation's marks.

use std::collections::HashMap;

use async_trait::async_trait;
use nullradix_domain::Timestamp;
use strum::{EnumString, IntoStaticStr};
use uuid::Uuid;

use crate::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum TrackKind {
    Research,
    Planning,
    AutopilotResearch,
}

impl TrackKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// One row: starts so far, the last start and when the automation took the subject up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tracked {
    pub count: u32,
    pub last_started_at: Option<Timestamp>,
    pub auto_at: Option<Timestamp>,
}

#[async_trait]
pub trait AutoRunRepository: Send + Sync {
    async fn get(&self, kind: TrackKind, subject: Uuid) -> Result<Option<Tracked>, AppError>;
    /// Every row of the kind by subject.
    async fn all(&self, kind: TrackKind) -> Result<HashMap<Uuid, Tracked>, AppError>;
    /// Counts a start at `at`; with `auto` the row also records that the automation took it up.
    async fn started(
        &self,
        kind: TrackKind,
        subject: Uuid,
        at: Timestamp,
        auto: bool,
    ) -> Result<(), AppError>;
    /// Marks a subject as taken up by the automation without a start; an earlier mark is replaced.
    async fn mark(&self, kind: TrackKind, subject: Uuid, at: Timestamp) -> Result<(), AppError>;
    /// Counts one more attempt without a start and returns the count.
    async fn bump(&self, kind: TrackKind, subject: Uuid) -> Result<u32, AppError>;
    /// Writes the row as given.
    async fn put(&self, kind: TrackKind, subject: Uuid, row: Tracked) -> Result<(), AppError>;
    async fn clear(&self, kind: TrackKind, subject: Uuid) -> Result<(), AppError>;
}
