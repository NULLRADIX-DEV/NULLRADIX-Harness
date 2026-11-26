//! Dispatcher statistics over a range of days, read from the runs.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Board, DispatchStats, Id, RunKind, STATS_MAX_RUNS, StatsError, StatsFacts, StatsRange,
    StatsRun, Timestamp, calculate_stats, local_day_of, parse_stats_kind, wants_config,
};

use super::local_time::LocalTime;
use crate::boards::{self, BoardRepository};
use crate::dispatcher_config::DispatcherConfigService;
use crate::{AppError, Clock};

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
    /// When the newest configuration in force up to the range end first appeared.
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

#[derive(Clone, Copy)]
enum Bound {
    Start,
    End,
}

fn refused(error: &StatsError) -> AppError {
    let code = match error {
        StatsError::BadKind => "stats-kind-invalid",
        _ => "stats-range-invalid",
    };
    AppError::invalid(code, error.to_string())
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

    /// A midnight bound of the range, with the offset in force at that midnight.
    async fn bound(&self, range: StatsRange, side: Bound, now: Timestamp) -> Timestamp {
        let at = |offset| match side {
            Bound::Start => range.start(offset),
            Bound::End => range.end(offset),
        };
        // the offset of the bound is read at the bound, guessed with today's offset first
        let guess = at(self.local.offset_minutes(now).await);
        at(self.local.offset_minutes(guess).await)
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
        let today = local_day_of(now, self.local.offset_minutes(now).await);
        let end = self
            .bound(
                StatsRange::resolve(request.from, request.to, today, None)
                    .map_err(|error| refused(&error))?,
                Bound::End,
                now,
            )
            .await;
        let config_changed_at = self
            .config
            .versions()
            .await?
            .iter()
            .rfind(|version| version.first_seen_at < end)
            .map(|version| version.first_seen_at);
        let config_day = match config_changed_at.filter(|_| wants_config(request.from)) {
            Some(at) => Some(local_day_of(at, self.local.offset_minutes(at).await)),
            None => None,
        };
        let range = StatsRange::resolve(request.from, request.to, today, config_day)
            .map_err(|error| refused(&error))?;
        let (from, until) = (
            self.bound(range, Bound::Start, now).await,
            self.bound(range, Bound::End, now).await,
        );
        let limit = u32::try_from(STATS_MAX_RUNS + 1).unwrap_or(u32::MAX);
        let mut runs = self
            .repository
            .runs(&StatsQuery {
                from,
                until,
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
        for run in &mut runs {
            run.offset_minutes = self.local.offset_minutes(run.started_at).await;
        }
        let stats = calculate_stats(&StatsFacts { kind, range, runs });
        Ok(StatsReport {
            board: request.board,
            range,
            kind,
            config_changed_at,
            stats,
        })
    }
}
