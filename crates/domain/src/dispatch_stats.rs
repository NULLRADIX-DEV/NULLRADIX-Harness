//! Dispatcher statistics: the range asked for and the numbers worked out from the runs.

use std::collections::{BTreeMap, HashMap};

use thiserror::Error;

use crate::timestamp::{civil_date, days_from_civil};
use crate::{AbortCategory, DispatchOutcome, RunKind, Timestamp};

/// Days a range covers when `from` is left out.
pub const STATS_DEFAULT_DAYS: i64 = 56;
/// Longest range in days.
pub const STATS_MAX_DAYS: i64 = 366;
/// Most runs a statistic reads; a longer list asks for a narrower range.
pub const STATS_MAX_RUNS: usize = 100_000;
/// Up to this many days the series has a point per day, beyond it one per week.
const DAILY_MAX_DAYS: i64 = 28;
const MINUTE_MS: i64 = 60_000;
const DAY_MINUTES: i64 = 24 * 60;

/// Why a range or a kind was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StatsError {
    #[error("{0} must be a date as YYYY-MM-DD")]
    BadDay(&'static str),
    #[error("from is after to")]
    FromAfterTo,
    #[error("the range is too long ({days} days, at most {STATS_MAX_DAYS})")]
    TooLong { days: i64 },
    #[error("kind must be work, merge or research")]
    BadKind,
}

/// Does `from` ask for the day the newest dispatcher configuration appeared?
#[must_use]
pub fn wants_config(from: Option<&str>) -> bool {
    from.map(str::trim) == Some("config")
}

fn filled(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.is_empty())
}

/// A calendar day as `YYYY-MM-DD` and back; the count is days since 1970-01-01.
fn parse_day(text: &str, field: &'static str) -> Result<i64, StatsError> {
    let text = text.trim();
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        });
    let number = |range: std::ops::Range<usize>| text[range].parse::<i64>().ok();
    let parts = shaped
        .then(|| Some((number(0..4)?, number(5..7)?, number(8..10)?)))
        .flatten();
    let (year, month, day) = parts.ok_or(StatsError::BadDay(field))?;
    let days = days_from_civil(year, month, day);
    // a date that reads back differently, such as February 30th, is no date
    if civil_date(days) != (year, month, day) {
        return Err(StatsError::BadDay(field));
    }
    Ok(days)
}

/// A day count as `YYYY-MM-DD`.
#[must_use]
pub fn day_label(days: i64) -> String {
    let (year, month, day) = civil_date(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The local day of a moment, for a fixed offset from UTC in minutes.
#[must_use]
pub fn local_day_of(at: Timestamp, offset_minutes: i32) -> i64 {
    (at.as_millis().div_euclid(MINUTE_MS) + i64::from(offset_minutes)).div_euclid(DAY_MINUTES)
}

fn day_start(day: i64, offset_minutes: i32) -> Timestamp {
    Timestamp::from_millis(
        day.saturating_mul(DAY_MINUTES * MINUTE_MS)
            .saturating_sub(i64::from(offset_minutes) * MINUTE_MS),
    )
}

/// The run kind of a statistic; work when left out, planning runs are not counted.
pub fn parse_stats_kind(text: Option<&str>) -> Result<RunKind, StatsError> {
    let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(RunKind::Work);
    };
    [RunKind::Work, RunKind::Merge, RunKind::Research]
        .into_iter()
        .find(|kind| kind.as_str() == text)
        .ok_or(StatsError::BadKind)
}

/// The days a statistic covers, first and last included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsRange {
    first: i64,
    last: i64,
}

impl StatsRange {
    /// Days from `from` to `to`; `config_day` stands for `from=config`; at most 366 days.
    pub fn resolve(
        from: Option<&str>,
        to: Option<&str>,
        today: i64,
        config_day: Option<i64>,
    ) -> Result<Self, StatsError> {
        let last = match filled(to) {
            Some(text) => parse_day(text, "to")?,
            None => today,
        };
        let fallback = last - (STATS_DEFAULT_DAYS - 1);
        let earliest = last - (STATS_MAX_DAYS - 1);
        let first = match (config_day, filled(from)) {
            (Some(changed), _) => changed.max(earliest),
            (None, Some(text)) if text.trim() != "config" => parse_day(text, "from")?,
            _ => fallback,
        };
        if first > last {
            return Err(StatsError::FromAfterTo);
        }
        let days = last - first + 1;
        if days > STATS_MAX_DAYS {
            return Err(StatsError::TooLong { days });
        }
        Ok(Self { first, last })
    }

    #[must_use]
    pub const fn first_day(self) -> i64 {
        self.first
    }

    #[must_use]
    pub const fn last_day(self) -> i64 {
        self.last
    }

    #[must_use]
    pub const fn days(self) -> i64 {
        self.last - self.first + 1
    }

    /// Midnight that starts the first day.
    #[must_use]
    pub fn start(self, offset_minutes: i32) -> Timestamp {
        day_start(self.first, offset_minutes)
    }

    /// Midnight after the last day; runs from it on are outside.
    #[must_use]
    pub fn end(self, offset_minutes: i32) -> Timestamp {
        day_start(self.last + 1, offset_minutes)
    }
}

/// What the store knows of one dispatch run.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsRun {
    /// Ticket or question key.
    pub subject: String,
    pub kind: RunKind,
    pub profile: String,
    pub outcome: Option<DispatchOutcome>,
    pub abort_category: Option<AbortCategory>,
    /// An earlier run of the subject was aborted, so this one is a retry or an escalation.
    pub retried: bool,
    pub started_at: Timestamp,
    /// Minutes the local day was ahead of UTC when the run started; the store leaves 0.
    pub offset_minutes: i32,
    pub duration_ms: Option<i64>,
    pub cost_usd: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
}

/// Everything the numbers are worked out from.
#[derive(Debug, Clone)]
pub struct StatsFacts {
    pub kind: RunKind,
    pub range: StatsRange,
    /// Runs of every kind that started inside the range.
    pub runs: Vec<StatsRun>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesUnit {
    Day,
    Week,
}

impl SeriesUnit {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesPoint {
    /// First day of the bucket as `YYYY-MM-DD`; a Monday for weeks.
    pub start: String,
    pub done: u32,
    pub aborted: u32,
    /// Waiting for a person: question or blocked.
    pub other: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsSeries {
    pub unit: SeriesUnit,
    pub points: Vec<SeriesPoint>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatsKpis {
    pub runs: u32,
    /// Tickets or questions the runs were about.
    pub subjects: u32,
    pub done: u32,
    pub aborted: u32,
    /// Subjects with at least one successful run.
    pub solved: u32,
    pub success_rate: Option<f64>,
    pub first_try_rate: Option<f64>,
    pub escalation_rate: Option<f64>,
    pub final_failures: u32,
}

/// Cost, tokens and time of the runs; not part of the legacy numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsUsage {
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// Mean over the runs that have a duration.
    pub average_duration_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileRow {
    pub profile: String,
    pub runs: u32,
    pub done: u32,
    /// Aborted, and tried again.
    pub escalated: u32,
    pub final_failures: u32,
    pub other: u32,
}

/// Finished subjects: solved at the first try, solved after an abort, failed for good.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsLadder {
    pub finished: u32,
    pub first_try: Option<f64>,
    pub after_escalation: Option<f64>,
    pub final_failures: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbortReason {
    /// The category as the wire spells it, `other` when the run recorded none.
    pub category: String,
    pub label: String,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeCount {
    pub outcome: DispatchOutcome,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DispatchStats {
    pub kind: RunKind,
    pub kpis: StatsKpis,
    pub usage: StatsUsage,
    pub series: StatsSeries,
    pub by_profile: Vec<ProfileRow>,
    pub ladder: StatsLadder,
    pub abort_reasons: Vec<AbortReason>,
    /// Share of the aborts that the configuration can change: turns, time and context.
    pub configurable_abort_share: Option<f64>,
    pub outcomes: Vec<OutcomeCount>,
    /// Pauses for a usage limit, runs of every kind.
    pub usage_pauses: u32,
    /// Runs the worker lost, runs of every kind.
    pub stale_runs: u32,
}

/// Name of an abort category for people.
#[must_use]
pub const fn abort_label(category: Option<AbortCategory>) -> &'static str {
    match category {
        Some(AbortCategory::Timeout) => "Time limit",
        Some(AbortCategory::MaxTurns) => "Turn limit",
        Some(AbortCategory::Context) => "Context full",
        Some(AbortCategory::ExitCode) => "Exit code",
        Some(AbortCategory::ClaudeError) => "Claude error",
        Some(AbortCategory::UsageLimit) => "Usage limit",
        Some(AbortCategory::StartFailed) => "Start failed",
        Some(AbortCategory::Vanished) => "Worker vanished",
        None => "Other",
    }
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn rate(part: u32, total: u32) -> Option<f64> {
    (total != 0).then(|| f64::from(part) / f64::from(total))
}

fn is_success(run: &StatsRun) -> bool {
    matches!(
        run.outcome,
        Some(
            DispatchOutcome::Done
                | DispatchOutcome::Review
                | DispatchOutcome::Human
                | DispatchOutcome::MergeSession
                | DispatchOutcome::Research
        )
    )
}

fn is_aborted(run: &StatsRun) -> bool {
    matches!(
        run.outcome,
        Some(DispatchOutcome::Abort | DispatchOutcome::AbortFinal)
    )
}

fn is_waiting(run: &StatsRun) -> bool {
    matches!(
        run.outcome,
        Some(DispatchOutcome::Question | DispatchOutcome::Blocked)
    )
}

fn is_final(run: &StatsRun) -> bool {
    run.outcome == Some(DispatchOutcome::AbortFinal)
}

/// The runs of each subject, oldest first.
fn by_subject<'a>(runs: &[&'a StatsRun]) -> BTreeMap<&'a str, Vec<&'a StatsRun>> {
    let mut sorted = runs.to_vec();
    sorted.sort_by_key(|run| run.started_at);
    let mut map: BTreeMap<&str, Vec<&StatsRun>> = BTreeMap::new();
    for run in sorted {
        map.entry(run.subject.as_str()).or_default().push(run);
    }
    map
}

fn ladder(subjects: &BTreeMap<&str, Vec<&StatsRun>>) -> StatsLadder {
    let (mut first_try, mut after_escalation, mut final_failures) = (0, 0, 0);
    for list in subjects.values() {
        match list.iter().position(|run| is_success(run)) {
            Some(success) => {
                let clean = !list[..=success]
                    .iter()
                    .any(|run| is_aborted(run) || run.retried);
                if clean {
                    first_try += 1;
                } else {
                    after_escalation += 1;
                }
            }
            None if list.iter().any(|run| is_final(run)) => final_failures += 1,
            None => {}
        }
    }
    let finished = first_try + after_escalation + final_failures;
    StatsLadder {
        finished,
        first_try: rate(first_try, finished),
        after_escalation: rate(after_escalation, finished),
        final_failures: rate(final_failures, finished),
    }
}

/// Day of a week bucket: the Monday on or before it; 1970-01-01 was a Thursday.
const fn monday(day: i64) -> i64 {
    day - (day + 3).rem_euclid(7)
}

fn series(facts: &StatsFacts, runs: &[&StatsRun]) -> StatsSeries {
    let (first, last) = (facts.range.first_day(), facts.range.last_day());
    let unit = if facts.range.days() <= DAILY_MAX_DAYS {
        SeriesUnit::Day
    } else {
        SeriesUnit::Week
    };
    let (step, start_of): (i64, fn(i64) -> i64) = match unit {
        SeriesUnit::Day => (1, |day| day),
        SeriesUnit::Week => (7, monday),
    };
    let mut buckets: BTreeMap<i64, (u32, u32, u32)> = BTreeMap::new();
    let mut day = start_of(first);
    while day <= last {
        buckets.insert(day, (0, 0, 0));
        day += step;
    }
    for run in runs {
        let bucket = start_of(local_day_of(run.started_at, run.offset_minutes));
        let Some(counts) = buckets.get_mut(&bucket) else {
            continue;
        };
        if is_success(run) {
            counts.0 += 1;
        } else if is_aborted(run) {
            counts.1 += 1;
        } else if is_waiting(run) {
            counts.2 += 1;
        }
    }
    StatsSeries {
        unit,
        points: buckets
            .into_iter()
            .map(|(day, (done, aborted, other))| SeriesPoint {
                start: day_label(day),
                done,
                aborted,
                other,
            })
            .collect(),
    }
}

/// Outcomes per profile, the busiest first.
fn profiles(runs: &[&StatsRun]) -> Vec<ProfileRow> {
    let mut rows: Vec<ProfileRow> = Vec::new();
    for run in runs {
        let index = rows
            .iter()
            .position(|row| row.profile == run.profile)
            .unwrap_or_else(|| {
                rows.push(ProfileRow {
                    profile: run.profile.clone(),
                    runs: 0,
                    done: 0,
                    escalated: 0,
                    final_failures: 0,
                    other: 0,
                });
                rows.len() - 1
            });
        let row = &mut rows[index];
        row.runs += 1;
        if is_success(run) {
            row.done += 1;
        } else if run.outcome == Some(DispatchOutcome::Abort) {
            row.escalated += 1;
        } else if is_final(run) {
            row.final_failures += 1;
        } else {
            row.other += 1;
        }
    }
    rows.sort_by(|a, b| b.runs.cmp(&a.runs).then_with(|| a.profile.cmp(&b.profile)));
    rows
}

fn abort_reasons(runs: &[&StatsRun]) -> Vec<AbortReason> {
    let mut counts: HashMap<Option<AbortCategory>, u32> = HashMap::new();
    for run in runs.iter().filter(|run| is_aborted(run)) {
        *counts.entry(run.abort_category).or_insert(0) += 1;
    }
    let mut reasons: Vec<AbortReason> = counts
        .into_iter()
        .map(|(category, count)| AbortReason {
            category: category.map_or("other", AbortCategory::as_str).to_owned(),
            label: abort_label(category).to_owned(),
            count,
        })
        .collect();
    reasons.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.category.cmp(&b.category))
    });
    reasons
}

fn outcomes(runs: &[&StatsRun]) -> Vec<OutcomeCount> {
    let mut counts: BTreeMap<&'static str, (DispatchOutcome, u32)> = BTreeMap::new();
    for outcome in runs.iter().filter_map(|run| run.outcome) {
        counts.entry(outcome.as_str()).or_insert((outcome, 0)).1 += 1;
    }
    let mut rows: Vec<OutcomeCount> = counts
        .into_values()
        .map(|(outcome, count)| OutcomeCount { outcome, count })
        .collect();
    rows.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.outcome.as_str().cmp(b.outcome.as_str()))
    });
    rows
}

fn usage(runs: &[&StatsRun]) -> StatsUsage {
    let sum = |value: fn(&StatsRun) -> Option<i64>| -> i64 {
        runs.iter()
            .filter_map(|run| value(run))
            .fold(0_i64, i64::saturating_add)
    };
    let durations: Vec<i64> = runs.iter().filter_map(|run| run.duration_ms).collect();
    let average = i64::try_from(durations.len())
        .ok()
        .filter(|length| *length > 0)
        .map(|length| durations.iter().copied().fold(0_i64, i64::saturating_add) / length);
    StatsUsage {
        cost_usd: runs.iter().filter_map(|run| run.cost_usd).sum(),
        input_tokens: sum(|run| run.input_tokens),
        output_tokens: sum(|run| run.output_tokens),
        cache_read_tokens: sum(|run| run.cache_read_tokens),
        cache_write_tokens: sum(|run| run.cache_write_tokens),
        average_duration_ms: average,
    }
}

/// Works the numbers out of the runs of the kind asked for.
#[must_use]
pub fn calculate_stats(facts: &StatsFacts) -> DispatchStats {
    let runs: Vec<&StatsRun> = facts
        .runs
        .iter()
        .filter(|run| run.kind == facts.kind)
        .collect();
    let subjects = by_subject(&runs);
    let ladder = ladder(&subjects);
    let aborted = count(runs.iter().filter(|run| is_aborted(run)).count());
    let solved = count(
        subjects
            .values()
            .filter(|list| list.iter().any(|run| is_success(run)))
            .count(),
    );
    let escalated = count(
        subjects
            .values()
            .filter(|list| list.iter().any(|run| run.retried))
            .count(),
    );
    let abort_reasons = abort_reasons(&runs);
    let configurable: u32 = abort_reasons
        .iter()
        .filter(|reason| ["max-turns", "timeout", "context"].contains(&reason.category.as_str()))
        .map(|reason| reason.count)
        .sum();
    DispatchStats {
        kind: facts.kind,
        kpis: StatsKpis {
            runs: count(runs.len()),
            subjects: count(subjects.len()),
            done: count(runs.iter().filter(|run| is_success(run)).count()),
            aborted,
            solved,
            success_rate: rate(solved, ladder.finished),
            first_try_rate: ladder.first_try,
            escalation_rate: rate(escalated, count(subjects.len())),
            final_failures: count(runs.iter().filter(|run| is_final(run)).count()),
        },
        usage: usage(&runs),
        series: series(facts, &runs),
        by_profile: profiles(&runs),
        ladder,
        abort_reasons,
        configurable_abort_share: rate(configurable, aborted),
        outcomes: outcomes(&runs),
        usage_pauses: count(
            facts
                .runs
                .iter()
                .filter(|run| run.outcome == Some(DispatchOutcome::PauseLimit))
                .count(),
        ),
        stale_runs: count(
            facts
                .runs
                .iter()
                .filter(|run| run.abort_category == Some(AbortCategory::Vanished))
                .count(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> i64 {
        parse_day(text, "day").unwrap()
    }

    fn range(from: Option<&str>, to: Option<&str>, today: &str) -> Result<StatsRange, StatsError> {
        StatsRange::resolve(from, to, day(today), None)
    }

    #[test]
    fn without_input_the_range_is_the_last_56_days_up_to_today() {
        let found = range(None, None, "2026-03-10").unwrap();
        assert_eq!(day_label(found.first_day()), "2026-01-14");
        assert_eq!(day_label(found.last_day()), "2026-03-10");
        assert_eq!(found.days(), STATS_DEFAULT_DAYS);
        assert_eq!(
            found.end(0).as_millis() - found.start(0).as_millis(),
            56 * 86_400_000
        );
    }

    #[test]
    fn config_starts_on_the_day_of_the_version_but_at_most_366_days_back() {
        let today = day("2026-03-10");
        let recent = StatsRange::resolve(Some("config"), None, today, Some(day("2026-03-07")));
        assert_eq!(day_label(recent.unwrap().first_day()), "2026-03-07");
        let old =
            StatsRange::resolve(Some("config"), None, today, Some(day("2020-01-01"))).unwrap();
        assert_eq!(day_label(old.first_day()), "2025-03-10");
        assert_eq!(old.days(), STATS_MAX_DAYS);
        let none = StatsRange::resolve(Some("config"), None, today, None).unwrap();
        assert_eq!(day_label(none.first_day()), "2026-01-14");
        assert!(wants_config(Some(" config ")));
        assert!(!wants_config(Some("2026-01-01")));
        assert!(!wants_config(None));
    }

    #[test]
    fn days_are_checked_as_calendar_days() {
        let ok = range(Some("2024-02-29"), Some("2024-03-01"), "2024-03-10").unwrap();
        assert_eq!(day_label(ok.first_day()), "2024-02-29");
        for text in [
            "2026-02-30",
            "2026-13-01",
            "2026-1-01",
            "today",
            "2026-02-28T00",
        ] {
            assert_eq!(
                range(None, Some(text), "2026-03-10"),
                Err(StatsError::BadDay("to")),
                "{text}"
            );
        }
        assert_eq!(
            range(Some("x"), None, "2026-03-10"),
            Err(StatsError::BadDay("from"))
        );
        // an empty value counts as not given
        assert_eq!(
            range(None, Some(""), "2026-03-10").unwrap().last_day(),
            day("2026-03-10")
        );
    }

    #[test]
    fn a_range_runs_forward_and_is_at_most_366_days() {
        assert_eq!(
            range(Some("2026-02-01"), Some("2026-01-01"), "2026-03-10"),
            Err(StatsError::FromAfterTo)
        );
        let too_long = range(Some("2025-01-01"), Some("2026-01-02"), "2026-03-10");
        assert_eq!(too_long, Err(StatsError::TooLong { days: 367 }));
        assert_eq!(
            too_long.unwrap_err().to_string(),
            "the range is too long (367 days, at most 366)"
        );
        assert!(range(Some("2025-01-02"), Some("2026-01-02"), "2026-03-10").is_ok());
    }

    #[test]
    fn midnights_follow_the_offset() {
        let found = range(Some("2026-03-10"), Some("2026-03-10"), "2026-03-10").unwrap();
        let utc = found.start(0).as_millis();
        assert_eq!(found.start(120).as_millis(), utc - 2 * 3_600_000);
        assert_eq!(found.start(-300).as_millis(), utc + 5 * 3_600_000);
        // 23:30 UTC is already the next day two hours east
        let late = Timestamp::from_millis(utc + 23 * 3_600_000 + 30 * 60_000);
        assert_eq!(local_day_of(late, 0), day("2026-03-10"));
        assert_eq!(local_day_of(late, 120), day("2026-03-11"));
    }

    #[test]
    fn the_kind_defaults_to_work_and_names_only_three() {
        assert_eq!(parse_stats_kind(None), Ok(RunKind::Work));
        assert_eq!(parse_stats_kind(Some(" ")), Ok(RunKind::Work));
        assert_eq!(parse_stats_kind(Some("merge")), Ok(RunKind::Merge));
        assert_eq!(parse_stats_kind(Some("research")), Ok(RunKind::Research));
        assert_eq!(parse_stats_kind(Some("plan")), Err(StatsError::BadKind));
    }

    const HOUR: i64 = 3_600_000;

    fn run(subject: &str, hour: i64, outcome: Option<DispatchOutcome>) -> StatsRun {
        StatsRun {
            subject: subject.to_owned(),
            kind: RunKind::Work,
            profile: "small".to_owned(),
            outcome,
            abort_category: None,
            retried: false,
            started_at: Timestamp::from_millis(day("2026-03-10") * 86_400_000 + hour * HOUR),
            offset_minutes: 0,
            duration_ms: None,
            cost_usd: None,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
        }
    }

    fn aborted(subject: &str, hour: i64, category: AbortCategory, last: bool) -> StatsRun {
        let outcome = if last {
            DispatchOutcome::AbortFinal
        } else {
            DispatchOutcome::Abort
        };
        StatsRun {
            abort_category: Some(category),
            ..run(subject, hour, Some(outcome))
        }
    }

    fn facts(runs: Vec<StatsRun>) -> StatsFacts {
        StatsFacts {
            kind: RunKind::Work,
            range: range(Some("2026-03-04"), Some("2026-03-10"), "2026-03-10").unwrap(),
            runs,
        }
    }

    #[test]
    fn no_runs_give_zeros_and_no_rates() {
        let stats = calculate_stats(&facts(Vec::new()));
        assert_eq!(stats.kpis.runs, 0);
        assert_eq!(stats.kpis.success_rate, None);
        assert_eq!(stats.kpis.first_try_rate, None);
        assert_eq!(stats.kpis.escalation_rate, None);
        assert_eq!(stats.configurable_abort_share, None);
        assert_eq!(stats.ladder.finished, 0);
        assert_eq!(stats.usage.average_duration_ms, None);
        assert!(stats.by_profile.is_empty() && stats.abort_reasons.is_empty());
        assert_eq!(stats.series.points.len(), 7);
    }

    #[test]
    fn the_ladder_tells_first_try_escalation_and_failure_apart() {
        let mut retry = run("ARC-2", 2, Some(DispatchOutcome::Done));
        retry.retried = true;
        let stats = calculate_stats(&facts(vec![
            run("ARC-1", 1, Some(DispatchOutcome::Done)),
            aborted("ARC-2", 1, AbortCategory::MaxTurns, false),
            retry,
            aborted("ARC-3", 1, AbortCategory::Timeout, false),
            aborted("ARC-3", 2, AbortCategory::Timeout, true),
            run("ARC-4", 1, Some(DispatchOutcome::Question)),
        ]));
        assert_eq!(stats.ladder.finished, 3);
        assert_eq!(stats.ladder.first_try, rate(1, 3));
        assert_eq!(stats.ladder.after_escalation, rate(1, 3));
        assert_eq!(stats.ladder.final_failures, rate(1, 3));
        assert_eq!(stats.kpis.subjects, 4);
        assert_eq!(stats.kpis.solved, 2);
        assert_eq!(stats.kpis.success_rate, rate(2, 3));
        assert_eq!(stats.kpis.escalation_rate, rate(1, 4));
        assert_eq!(stats.kpis.done, 2);
        assert_eq!(stats.kpis.aborted, 3);
        assert_eq!(stats.kpis.final_failures, 1);
    }

    #[test]
    fn a_subject_without_success_or_final_abort_is_not_finished() {
        let stats = calculate_stats(&facts(vec![
            aborted("ARC-1", 1, AbortCategory::ExitCode, false),
            run("ARC-2", 1, None),
        ]));
        assert_eq!(stats.ladder.finished, 0);
        assert_eq!(stats.kpis.success_rate, None);
    }

    #[test]
    fn a_retry_before_the_range_makes_a_later_success_an_escalation() {
        let mut late = run("ARC-1", 1, Some(DispatchOutcome::Done));
        late.retried = true;
        let stats = calculate_stats(&facts(vec![late]));
        assert_eq!(stats.ladder.first_try, Some(0.0));
        assert_eq!(stats.ladder.after_escalation, Some(1.0));
    }

    #[test]
    fn aborts_are_split_by_category_with_the_configurable_share() {
        let mut none = aborted("ARC-4", 1, AbortCategory::Context, false);
        none.abort_category = None;
        let stats = calculate_stats(&facts(vec![
            aborted("ARC-1", 1, AbortCategory::MaxTurns, false),
            aborted("ARC-2", 1, AbortCategory::MaxTurns, false),
            aborted("ARC-3", 1, AbortCategory::ExitCode, true),
            none,
        ]));
        let reasons: Vec<(&str, &str, u32)> = stats
            .abort_reasons
            .iter()
            .map(|r| (r.category.as_str(), r.label.as_str(), r.count))
            .collect();
        assert_eq!(
            reasons,
            [
                ("max-turns", "Turn limit", 2),
                ("exit-code", "Exit code", 1),
                ("other", "Other", 1)
            ]
        );
        assert_eq!(stats.configurable_abort_share, rate(2, 4));
    }

    #[test]
    fn profiles_count_outcomes_busiest_first() {
        let mut big = run("ARC-1", 3, Some(DispatchOutcome::Review));
        big.profile = "large".to_owned();
        let stats = calculate_stats(&facts(vec![
            run("ARC-1", 1, Some(DispatchOutcome::Abort)),
            run("ARC-2", 1, Some(DispatchOutcome::Blocked)),
            aborted("ARC-3", 1, AbortCategory::Timeout, true),
            big,
        ]));
        assert_eq!(
            stats.by_profile,
            [
                ProfileRow {
                    profile: "small".into(),
                    runs: 3,
                    done: 0,
                    escalated: 1,
                    final_failures: 1,
                    other: 1
                },
                ProfileRow {
                    profile: "large".into(),
                    runs: 1,
                    done: 1,
                    escalated: 0,
                    final_failures: 0,
                    other: 0
                },
            ]
        );
    }

    #[test]
    fn a_run_that_never_started_counts_as_neither_done_nor_aborted() {
        let stats = calculate_stats(&facts(vec![
            run("ARC-1", 1, Some(DispatchOutcome::StartFailed)),
            run("ARC-2", 2, Some(DispatchOutcome::Done)),
        ]));
        assert_eq!(stats.kpis.runs, 2);
        assert_eq!(stats.by_profile[0].done, 1);
        assert_eq!(stats.by_profile[0].other, 1);
        assert_eq!(stats.abort_reasons, []);
        assert!(
            stats
                .outcomes
                .iter()
                .any(|row| { row.outcome == DispatchOutcome::StartFailed && row.count == 1 })
        );
    }
    #[test]
    fn only_the_kind_asked_for_counts_but_pauses_and_lost_runs_count_for_all() {
        let mut merge = run("ARC-1", 1, Some(DispatchOutcome::MergeSession));
        merge.kind = RunKind::Merge;
        let mut lost = aborted("ARC-2", 1, AbortCategory::Vanished, false);
        lost.kind = RunKind::Research;
        let mut paused = run("ARC-3", 1, Some(DispatchOutcome::PauseLimit));
        paused.kind = RunKind::Merge;
        let stats = calculate_stats(&facts(vec![
            merge,
            lost,
            paused,
            run("ARC-4", 1, Some(DispatchOutcome::Done)),
        ]));
        assert_eq!(stats.kpis.runs, 1);
        assert_eq!(stats.usage_pauses, 1);
        assert_eq!(stats.stale_runs, 1);
        let merges = calculate_stats(&StatsFacts {
            kind: RunKind::Merge,
            ..facts(vec![run("ARC-1", 1, Some(DispatchOutcome::MergeSession))])
        });
        assert_eq!(merges.kpis.runs, 0);
    }

    #[test]
    fn the_series_has_a_point_per_day_and_counts_by_local_day() {
        let mut stats_facts = facts(vec![
            run("ARC-1", 1, Some(DispatchOutcome::Done)),
            run("ARC-2", 23, Some(DispatchOutcome::Abort)),
            run("ARC-3", 5, Some(DispatchOutcome::Question)),
            run("ARC-4", 5, Some(DispatchOutcome::PauseLimit)),
        ]);
        let last = |stats: &DispatchStats| stats.series.points.last().cloned().unwrap();
        let stats = calculate_stats(&stats_facts);
        assert_eq!(stats.series.unit, SeriesUnit::Day);
        assert_eq!(
            last(&stats),
            SeriesPoint {
                start: "2026-03-10".into(),
                done: 1,
                aborted: 1,
                other: 1
            }
        );
        for run in &mut stats_facts.runs {
            run.offset_minutes = 120;
        }
        let shifted = calculate_stats(&stats_facts);
        // 23:00 UTC is 01:00 the next day, outside the range
        assert_eq!(last(&shifted).aborted, 0);
    }

    #[test]
    fn beyond_28_days_the_series_goes_by_weeks_from_monday() {
        let long = StatsFacts {
            range: range(Some("2026-01-14"), Some("2026-03-10"), "2026-03-10").unwrap(),
            ..facts(vec![
                run("ARC-1", 1, Some(DispatchOutcome::Done)),
                run("ARC-2", 1, Some(DispatchOutcome::Blocked)),
            ])
        };
        let stats = calculate_stats(&long);
        assert_eq!(stats.series.unit, SeriesUnit::Week);
        let week_days: Vec<&str> = stats
            .series
            .points
            .iter()
            .map(|p| p.start.as_str())
            .collect();
        assert_eq!(week_days.first(), Some(&"2026-01-12"));
        assert_eq!(week_days.last(), Some(&"2026-03-09"));
        assert_eq!(week_days.len(), 9);
        let last = stats.series.points.last().unwrap();
        assert_eq!((last.done, last.aborted, last.other), (1, 0, 1));
    }

    #[test]
    fn usage_adds_cost_and_tokens_and_averages_known_durations() {
        let mut first = run("ARC-1", 1, Some(DispatchOutcome::Done));
        first.cost_usd = Some(1.5);
        first.input_tokens = Some(100);
        first.output_tokens = Some(10);
        first.duration_ms = Some(1_000);
        let mut second = run("ARC-2", 1, Some(DispatchOutcome::Done));
        second.cost_usd = Some(0.25);
        second.input_tokens = Some(50);
        second.cache_read_tokens = Some(7);
        second.duration_ms = Some(3_000);
        let stats = calculate_stats(&facts(vec![first, second, run("ARC-3", 1, None)]));
        assert!((stats.usage.cost_usd - 1.75).abs() < 1e-9);
        assert_eq!(stats.usage.input_tokens, 150);
        assert_eq!(stats.usage.output_tokens, 10);
        assert_eq!(stats.usage.cache_read_tokens, 7);
        assert_eq!(stats.usage.cache_write_tokens, 0);
        assert_eq!(stats.usage.average_duration_ms, Some(2_000));
    }

    #[test]
    fn outcomes_are_counted_most_frequent_first() {
        let stats = calculate_stats(&facts(vec![
            run("ARC-1", 1, Some(DispatchOutcome::Done)),
            run("ARC-2", 1, Some(DispatchOutcome::Blocked)),
            run("ARC-3", 1, Some(DispatchOutcome::Blocked)),
            run("ARC-4", 1, None),
        ]));
        let rows: Vec<(&str, u32)> = stats
            .outcomes
            .iter()
            .map(|row| (row.outcome.as_str(), row.count))
            .collect();
        assert_eq!(rows, [("blocked", 2), ("done", 1)]);
    }

    #[test]
    fn weeks_start_on_monday() {
        assert_eq!(day_label(monday(day("2026-03-10"))), "2026-03-09");
        assert_eq!(day_label(monday(day("2026-03-09"))), "2026-03-09");
        assert_eq!(day_label(monday(day("2026-03-15"))), "2026-03-09");
        assert_eq!(day_label(monday(0)), "1969-12-29");
    }
}
