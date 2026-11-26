//! The dispatcher's runtime state: the switch, the pause and what the last pass did.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_domain::{DispatcherState, Timestamp, Version};
use uuid::Uuid;

use super::pass::PassReport;
use crate::{AppError, Change, ChangeSignal, Clock};

/// Entity id of the one dispatcher state in the change feed.
pub const STATE_ENTITY_ID: Uuid = Uuid::from_u128(0x019a_0000_0000_7000_8000_0000_0000_0004);

/// Longest pause a person may ask for, one day as in legacy.
pub const MAX_PAUSE_MINUTES: u32 = 1440;

/// How often a write retries when another one changed the state in between.
const WRITE_ATTEMPTS: usize = 5;

#[async_trait]
pub trait DispatcherStateRepository: Send + Sync {
    /// The stored state; a fresh store holds the defaults.
    async fn load(&self) -> Result<DispatcherState, AppError>;
    /// Stores `state` while the stored one is still at `expected`; `None` when it moved on.
    async fn save(
        &self,
        state: &DispatcherState,
        expected: Version,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
}

#[must_use]
pub fn pause_invalid(minutes: u32) -> AppError {
    AppError::invalid(
        "dispatcher-pause-invalid",
        format!("pause between 1 and {MAX_PAUSE_MINUTES} minutes, not {minutes}"),
    )
}

#[must_use]
pub fn pause_too_long() -> AppError {
    AppError::invalid(
        "dispatcher-pause-invalid",
        format!("the pause must end within {MAX_PAUSE_MINUTES} minutes"),
    )
}

#[must_use]
pub fn pause_in_the_past() -> AppError {
    AppError::invalid(
        "dispatcher-pause-invalid",
        "the pause must end in the future",
    )
}

pub struct DispatcherStateService {
    repository: Arc<dyn DispatcherStateRepository>,
    clock: Arc<dyn Clock>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for DispatcherStateService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatcherStateService")
            .finish_non_exhaustive()
    }
}

impl DispatcherStateService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn DispatcherStateRepository>,
        clock: Arc<dyn Clock>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            repository,
            clock,
            signal,
        }
    }

    pub async fn get(&self) -> Result<DispatcherState, AppError> {
        self.repository.load().await
    }

    /// Applies `next` to the state until it is stored; the result is the stored state.
    async fn change(
        &self,
        next: impl Fn(&DispatcherState) -> DispatcherState + Send,
    ) -> Result<DispatcherState, AppError> {
        for _ in 0..WRITE_ATTEMPTS {
            let current = self.repository.load().await?;
            let state = next(&current);
            if state == current {
                return Ok(current);
            }
            let saved = self
                .repository
                .save(&state, current.record().version, self.clock.now())
                .await?;
            if let Some(change) = saved {
                self.signal.notify(&change);
                return Ok(state);
            }
        }
        Err(AppError::conflict(
            "version-conflict",
            "the dispatcher state keeps changing, try again",
        ))
    }

    /// Sets the switch; `None` follows the configuration again.
    pub async fn set_enabled(&self, enabled: Option<bool>) -> Result<DispatcherState, AppError> {
        self.change(|state| {
            if state.record().enabled_override == enabled {
                state.clone()
            } else {
                state.with_enabled(enabled)
            }
        })
        .await
    }

    /// Pauses new starts for `minutes`, at least one and at most a day.
    pub async fn pause_for(&self, minutes: u32) -> Result<DispatcherState, AppError> {
        if !(1..=MAX_PAUSE_MINUTES).contains(&minutes) {
            return Err(pause_invalid(minutes));
        }
        let span = Duration::from_secs(u64::from(minutes) * 60);
        let until = self
            .clock
            .now()
            .checked_add(span)
            .ok_or_else(|| pause_invalid(minutes))?;
        self.pause_until(until).await
    }

    /// Pauses until a moment, which must lie in the future.
    pub async fn pause_until(&self, until: Timestamp) -> Result<DispatcherState, AppError> {
        let now = self.clock.now();
        if until <= now {
            return Err(pause_in_the_past());
        }
        let latest = now
            .checked_add(Duration::from_secs(u64::from(MAX_PAUSE_MINUTES) * 60))
            .ok_or_else(pause_too_long)?;
        if until > latest {
            return Err(pause_too_long());
        }
        self.change(|state| {
            if state.record().paused_until == Some(until) {
                state.clone()
            } else {
                state.paused(until)
            }
        })
        .await
    }

    /// Pauses until a moment, cut to the longest pause, unless the pause changed since `seen`.
    pub async fn pause_unless_changed(
        &self,
        until: Timestamp,
        seen: Option<Timestamp>,
    ) -> Result<bool, AppError> {
        let now = self.clock.now();
        let latest = now
            .checked_add(Duration::from_secs(u64::from(MAX_PAUSE_MINUTES) * 60))
            .ok_or_else(pause_too_long)?;
        let until = until.min(latest);
        if until <= now {
            return Err(pause_in_the_past());
        }
        let applied = std::sync::atomic::AtomicBool::new(true);
        self.change(|state| {
            let current = state.record().paused_until;
            applied.store(current == seen, std::sync::atomic::Ordering::SeqCst);
            if current != seen || current == Some(until) {
                state.clone()
            } else {
                state.paused(until)
            }
        })
        .await?;
        Ok(applied.load(std::sync::atomic::Ordering::SeqCst))
    }

    pub async fn resume(&self) -> Result<DispatcherState, AppError> {
        self.change(|state| {
            if state.record().paused_until.is_some() {
                state.resumed()
            } else {
                state.clone()
            }
        })
        .await
    }

    /// Keeps what a pass did, so the status survives a restart.
    pub async fn record_pass(&self, report: &PassReport) -> Result<(), AppError> {
        let text = report.to_json();
        let at = Timestamp::from_millis(report.at);
        self.change(|state| state.passed(at, text.clone())).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pauses_outside_the_range_have_a_stable_code() {
        assert_eq!(pause_invalid(0).code(), "dispatcher-pause-invalid");
        assert_eq!(pause_in_the_past().code(), "dispatcher-pause-invalid");
    }
}
