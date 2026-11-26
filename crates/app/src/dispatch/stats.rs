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

const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 24 * 60 * MINUTE_MS;

fn moment_plus(at: Timestamp, millis: i64) -> Timestamp {
    Timestamp::from_millis(at.as_millis().saturating_add(millis))
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

    /// A midnight bound of the range: the first moment of its local day.
    async fn bound(&self, range: StatsRange, side: Bound, today_offset: i32) -> Timestamp {
        let at = |offset| match side {
            Bound::Start => range.start(offset),
            Bound::End => range.end(offset),
        };
        let day = match side {
            Bound::Start => range.first_day(),
            Bound::End => range.last_day() + 1,
        };
        // zones that switch at midnight have two candidates; the right one starts the day
        let guess = at(today_offset);
        let around = [
            guess,
            moment_plus(guess, -DAY_MS),
            moment_plus(guess, DAY_MS),
        ];
        let mut candidates: Vec<Timestamp> = self
            .local
            .offsets(&around)
            .await
            .into_iter()
            .map(at)
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        let probes: Vec<Timestamp> = candidates
            .iter()
            .flat_map(|at| [*at, moment_plus(*at, -MINUTE_MS)])
            .collect();
        let offsets = self.local.offsets(&probes).await;
        candidates
            .iter()
            .enumerate()
            .find(|(index, at)| {
                let (Some(&here), Some(&before)) =
                    (offsets.get(index * 2), offsets.get(index * 2 + 1))
                else {
                    return false;
                };
                local_day_of(**at, here) == day
                    && local_day_of(moment_plus(**at, -MINUTE_MS), before) < day
            })
            .map(|(_, at)| *at)
            .or_else(|| candidates.last().copied())
            .unwrap_or(guess)
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
        let today_offset = self.local.offset_minutes(now).await;
        let today = local_day_of(now, today_offset);
        let end = self
            .bound(
                StatsRange::resolve(request.from, request.to, today, None)
                    .map_err(|error| refused(&error))?,
                Bound::End,
                today_offset,
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
            self.bound(range, Bound::Start, today_offset).await,
            self.bound(range, Bound::End, today_offset).await,
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
        let moments: Vec<Timestamp> = runs.iter().map(|run| run.started_at).collect();
        for (run, offset) in runs.iter_mut().zip(self.local.offsets(&moments).await) {
            run.offset_minutes = offset;
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
