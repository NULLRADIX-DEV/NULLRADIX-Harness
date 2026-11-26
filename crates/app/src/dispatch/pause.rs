//! The hook for runs that ended on a usage limit or without Claude; pauses are a later slice.

use async_trait::async_trait;
use nullradix_domain::{DispatchOutcome, DispatcherConfig, Run, TicketRecord};

use crate::AppError;

/// Why a run ended without a fault of the ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseKind {
    UsageLimit,
    ClaudeMissing,
}

/// Takes runs that ended on the usage limit or because Claude was missing.
#[async_trait]
pub trait PauseHook: Send + Sync {
    /// `Some` settles the run with that outcome and counts no attempt; `None` counts one.
    async fn paused(
        &self,
        run: &Run,
        ticket: &TicketRecord,
        kind: PauseKind,
        config: &DispatcherConfig,
    ) -> Result<Option<DispatchOutcome>, AppError>;
}

/// Pauses nothing yet, so these runs count as aborted attempts.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPauseHook;

#[async_trait]
impl PauseHook for NoPauseHook {
    async fn paused(
        &self,
        _run: &Run,
        _ticket: &TicketRecord,
        _kind: PauseKind,
        _config: &DispatcherConfig,
    ) -> Result<Option<DispatchOutcome>, AppError> {
        Ok(None)
    }
}
