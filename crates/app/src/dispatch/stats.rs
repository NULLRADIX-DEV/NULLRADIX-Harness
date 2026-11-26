//! Dispatcher statistics over a range of days, read from the runs.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Board, DispatchStats, Id, RunKind, STATS_MAX_RUNS, StatsError, StatsFacts, StatsRange,
    StatsRun, Timestamp, calculate_stats, local_day_of, parse_stats_kind, wants_config,
};

use super::local_time::{LocalTime, minute_with_offset};
use crate::boards::{self, BoardRepository};
use crate::dispatcher_config::DispatcherConfigService;
use crate::{AppError, Clock};

const DAY_MINUTES: i64 = 24 * 60;

/// Which runs to read.
#[derive(Debug, Clone, Copy)]
pub struct StatsQuery {
    /// Runs created from here on.
    pub from: Timestamp,
    /// Runs created before this moment.
    pub until: Timestamp,
    /// Only runs of tickets on this board; question runs belong to no board and always count.
    pub board: Option<Id<Board>>,
    /// Most rows to return; the newest are left out beyond it.
    pub limit: u32,
}

#[async_trait]
pub trait DispatchStatsRepository: Send + Sync {
    /// Dispatch runs of every kind, oldest first.
    async fn runs(&self, query: &StatsQuery) -> Result<Vec<StatsRun>, AppError>;
}

/// What the caller asks for; text as it came in.
#[derive(Debug, Clone, Copy, Default)]
pub struct StatsRequest<'a> {
    pub from: Option<&'a str>,
    pub to: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub board: Option<Id<Board>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatsReport {
    pub board: Option<Id<Board>>,
    pub range: StatsRange,
    pub kind: RunKind,
    /// When the newest dispatcher configuration first appeared.
    pub config_changed_at: Option<Timestamp>,
    pub stats: DispatchStats,
}

pub struct DispatchStatsService {
    repository: Arc<dyn DispatchStatsRepository>,
    config: Arc<DispatcherConfigService>,
    boards: Arc<dyn BoardRepository>,
    clock: Arc<dyn Clock>,
    local: Arc<dyn LocalTime>,
}

impl std::fmt::Debug for DispatchStatsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchStatsService")
            .finish_non_exhaustive()
    }
}

fn refused(error: &StatsError) -> AppError {
    let code = match error {
        StatsError::BadKind => "stats-kind-invalid",
        _ => "stats-range-invalid",
    };
    AppError::invalid(code, error.to_string())
}

/// Minutes the local clock is ahead of UTC at `now`, from -12 to +14 hours.
async fn offset_minutes(local: &dyn LocalTime, now: Timestamp) -> i32 {
    let ahead = i64::from(local.minute_of_day(now).await) - i64::from(minute_with_offset(now, 0));
    let mut offset = ahead.rem_euclid(DAY_MINUTES);
    if offset > 14 * 60 {
        offset -= DAY_MINUTES;
    }
    i32::try_from(offset).unwrap_or(0)
}

impl DispatchStatsService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn DispatchStatsRepository>,
        config: Arc<DispatcherConfigService>,
        boards: Arc<dyn BoardRepository>,
        clock: Arc<dyn Clock>,
        local: Arc<dyn LocalTime>,
    ) -> Self {
        Self {
            repository,
            config,
            boards,
            clock,
            local,
        }
    }

    /// The numbers of the runs that started on the days asked for, in local time.
    pub async fn get(&self, request: StatsRequest<'_>) -> Result<StatsReport, AppError> {
        let kind = parse_stats_kind(request.kind).map_err(|error| refused(&error))?;
        if let Some(id) = request.board
            && self.boards.get(id).await?.is_none()
        {
            return Err(boards::not_found(id));
        }
        let now = self.clock.now();
        let offset = offset_minutes(self.local.as_ref(), now).await;
        let config_changed_at = if wants_config(request.from) {
            self.config
                .versions()
                .await?
                .last()
                .map(|version| version.first_seen_at)
        } else {
            None
        };
        let range = StatsRange::resolve(
            request.from,
            request.to,
            local_day_of(now, offset),
            config_changed_at.map(|at| local_day_of(at, offset)),
        )
        .map_err(|error| refused(&error))?;
        let limit = u32::try_from(STATS_MAX_RUNS + 1).unwrap_or(u32::MAX);
        let runs = self
            .repository
            .runs(&StatsQuery {
                from: range.start(offset),
                until: range.end(offset),
                board: request.board,
                limit,
            })
            .await?;
        if runs.len() > STATS_MAX_RUNS {
            return Err(AppError::invalid(
                "stats-range-too-busy",
                format!("more than {STATS_MAX_RUNS} runs in the range, choose fewer days"),
            ));
        }
        let stats = calculate_stats(&StatsFacts {
            kind,
            range,
            offset_minutes: offset,
            runs,
        });
        Ok(StatsReport {
            board: request.board,
            range,
            kind,
            config_changed_at,
            stats,
        })
    }
}
