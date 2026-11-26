use std::collections::HashMap;

use crate::{Account, Id, Timestamp};

use super::account_choice::{AccountReserve, AccountState, UsageSnapshot};

/// From this many fitting samples the percentile replaces the fixed buffer.
pub const MIN_SAMPLES: usize = 5;
/// At most this many of the latest fitting samples count.
pub const MAX_SAMPLES: usize = 20;

/// A finished run with the use of its account at the start and at the end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSample {
    pub profile: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub started_at: Timestamp,
    pub ended_at: Timestamp,
    pub before: UsageSnapshot,
    pub after: UsageSnapshot,
}

/// What a reserve is estimated for.
#[derive(Debug, Clone, Copy)]
pub struct ReserveFor<'a> {
    pub profile: &'a str,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstimatedReserve {
    pub reserve: AccountReserve,
    /// Where the reserve comes from, for the reason text.
    pub source: String,
    /// Expected minutes of the run, rounded up; none with too few samples.
    pub minutes: Option<i64>,
}

/// The reserve of a run from past samples (oldest first); `fallback` is the fixed buffer.
#[must_use]
pub fn estimate_reserve(
    samples: &[UsageSample],
    run: &ReserveFor<'_>,
    fallback: AccountReserve,
) -> EstimatedReserve {
    let same_profile: Vec<&UsageSample> = samples
        .iter()
        .filter(|s| s.profile == run.profile)
        .collect();
    let same_all: Vec<&UsageSample> = same_profile
        .iter()
        .copied()
        .filter(|s| s.model.as_deref() == run.model && s.effort.as_deref() == run.effort)
        .collect();
    let five = estimate_window(&same_all, &same_profile, run, FIVE_HOUR, fallback.five_hour);
    let week = estimate_window(&same_all, &same_profile, run, WEEK, fallback.week);
    EstimatedReserve {
        reserve: AccountReserve {
            five_hour: five.0,
            week: week.0,
        },
        source: five.1,
        minutes: estimate_minutes(&same_all, &same_profile),
    }
}

/// 75th percentile of the latest fitting durations; too few samples give none.
fn estimate_minutes(same_all: &[&UsageSample], same_profile: &[&UsageSample]) -> Option<i64> {
    [same_all, same_profile].into_iter().find_map(|candidates| {
        let seconds: Vec<i64> = candidates.iter().filter_map(|s| run_seconds(s)).collect();
        let recent = &seconds[seconds.len().saturating_sub(MAX_SAMPLES)..];
        (recent.len() >= MIN_SAMPLES).then(|| (percentile_75(recent) + 59) / 60)
    })
}

fn run_seconds(sample: &UsageSample) -> Option<i64> {
    let millis = sample
        .ended_at
        .as_millis()
        .checked_sub(sample.started_at.as_millis())?;
    (millis >= 0).then_some(millis / 1000)
}

type Window = fn(&UsageSample) -> Option<i64>;

const FIVE_HOUR: Window = |s| {
    window_usage(
        s,
        (s.before.five_hour, s.after.five_hour),
        s.before.five_hour_resets_at,
    )
};
const WEEK: Window = |s| {
    window_usage(
        s,
        (s.before.seven_day, s.after.seven_day),
        s.before.seven_day_resets_at,
    )
};

/// Use in one window; none when unknown, negative, or the window reset during the run.
fn window_usage(
    sample: &UsageSample,
    (before, after): (Option<i64>, Option<i64>),
    resets_at: Option<Timestamp>,
) -> Option<i64> {
    let used = after? - before?;
    (used >= 0 && resets_at? > sample.ended_at).then_some(used)
}

/// The reserve of one window and its source: profile, model and effort, then profile, then buffer.
fn estimate_window(
    same_all: &[&UsageSample],
    same_profile: &[&UsageSample],
    run: &ReserveFor<'_>,
    window: Window,
    fallback: i64,
) -> (i64, String) {
    let tiers = [
        (
            same_all,
            format!(
                "profile:{}/{}/{}",
                run.profile,
                run.model.unwrap_or("-"),
                run.effort.unwrap_or("-")
            ),
        ),
        (same_profile, format!("profile {}", run.profile)),
    ];
    for (candidates, what) in tiers {
        let used: Vec<i64> = candidates.iter().filter_map(|s| window(s)).collect();
        let recent = &used[used.len().saturating_sub(MAX_SAMPLES)..];
        if recent.len() >= MIN_SAMPLES {
            return (
                percentile_75(recent),
                format!("from {} runs {what}", recent.len()),
            );
        }
    }
    (fallback, String::from("buffer"))
}

/// Nearest rank, rounded up; `values` is never empty.
fn percentile_75(values: &[i64]) -> i64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = (sorted.len() * 3).div_ceil(4);
    sorted[rank - 1]
}

/// A work run that has not ended, with the use of its account when it started.
#[derive(Debug, Clone)]
pub struct RunningWork {
    pub account: Id<Account>,
    pub profile: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub start: UsageSnapshot,
}

/// Use still to come from runs under way per account: their reserve minus use since the start.
#[must_use]
pub fn pending_reserve<S: std::hash::BuildHasher>(
    running: &[RunningWork],
    samples: &[UsageSample],
    fallback: AccountReserve,
    states: &HashMap<Id<Account>, AccountState, S>,
) -> HashMap<Id<Account>, AccountReserve> {
    let mut pending: HashMap<Id<Account>, AccountReserve> = HashMap::new();
    for work in running {
        let estimate = estimate_reserve(
            samples,
            &ReserveFor {
                profile: &work.profile,
                model: work.model.as_deref(),
                effort: work.effort.as_deref(),
            },
            fallback,
        );
        let now = states.get(&work.account).map(|state| state.usage);
        let used = |now: Option<i64>, start: Option<i64>| match (now, start) {
            (Some(now), Some(start)) => (now - start).max(0),
            _ => 0,
        };
        let rest = AccountReserve {
            five_hour: (estimate.reserve.five_hour
                - used(now.and_then(|u| u.five_hour), work.start.five_hour))
            .max(0),
            week: (estimate.reserve.week
                - used(now.and_then(|u| u.seven_day), work.start.seven_day))
            .max(0),
        };
        let total = pending.entry(work.account).or_default();
        *total = total.plus(rest);
    }
    pending
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_800_000_000_000;
    const HOUR: i64 = 3_600_000;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis)
    }

    fn snapshot(five: i64, week: i64) -> UsageSnapshot {
        UsageSnapshot {
            five_hour: Some(five),
            seven_day: Some(week),
            five_hour_resets_at: Some(at(T0 + 4 * HOUR)),
            seven_day_resets_at: Some(at(T0 + 48 * HOUR)),
        }
    }

    fn sample(profile: &str, five: i64, week: i64, minutes: i64) -> UsageSample {
        UsageSample {
            profile: profile.to_owned(),
            model: Some(String::from("sonnet")),
            effort: Some(String::from("high")),
            started_at: at(T0),
            ended_at: at(T0 + minutes * 60_000),
            before: snapshot(10, 20),
            after: snapshot(10 + five, 20 + week),
        }
    }

    fn run<'a>(profile: &'a str, model: &'a str) -> ReserveFor<'a> {
        ReserveFor {
            profile,
            model: Some(model),
            effort: Some("high"),
        }
    }

    const BUFFER: AccountReserve = AccountReserve {
        five_hour: 10,
        week: 2,
    };

    #[test]
    fn too_few_samples_use_the_buffer() {
        let samples: Vec<_> = (0..4).map(|n| sample("small", n, 1, 10)).collect();
        let estimate = estimate_reserve(&samples, &run("small", "sonnet"), BUFFER);
        assert_eq!(estimate.reserve, BUFFER);
        assert_eq!(estimate.source, "buffer");
        assert_eq!(estimate.minutes, None);
    }

    #[test]
    fn five_samples_give_the_75th_percentile_by_nearest_rank() {
        let samples: Vec<_> = [1, 2, 3, 4, 20]
            .into_iter()
            .map(|n| sample("small", n, n, 10 * n))
            .collect();
        let estimate = estimate_reserve(&samples, &run("small", "sonnet"), BUFFER);
        // rank ceil(5 * 3 / 4) = 4
        assert_eq!(
            estimate.reserve,
            AccountReserve {
                five_hour: 4,
                week: 4
            }
        );
        assert_eq!(estimate.source, "from 5 runs profile:small/sonnet/high");
        assert_eq!(estimate.minutes, Some(40));
    }

    #[test]
    fn the_exact_tier_needs_five_samples_else_the_profile_tier_counts() {
        let mut samples: Vec<_> = (0..4).map(|_| sample("small", 6, 3, 10)).collect();
        samples.push(UsageSample {
            model: Some(String::from("opus")),
            ..sample("small", 6, 3, 10)
        });
        samples.push(sample("large", 50, 50, 10));
        let estimate = estimate_reserve(&samples, &run("small", "sonnet"), BUFFER);
        assert_eq!(estimate.source, "from 5 runs profile small");
        assert_eq!(estimate.reserve.five_hour, 6);
    }

    #[test]
    fn only_the_latest_twenty_samples_count() {
        let mut samples: Vec<_> = (0..10).map(|_| sample("small", 40, 9, 10)).collect();
        samples.extend((0..20).map(|_| sample("small", 3, 1, 10)));
        let estimate = estimate_reserve(&samples, &run("small", "sonnet"), BUFFER);
        assert_eq!(estimate.reserve.five_hour, 3);
        assert_eq!(estimate.source, "from 20 runs profile:small/sonnet/high");
    }

    #[test]
    fn a_window_that_reset_during_the_run_or_went_down_is_not_a_sample() {
        let reset = |n: i64| UsageSample {
            before: UsageSnapshot {
                five_hour_resets_at: Some(at(T0 + 5 * 60_000)),
                ..snapshot(10, 20)
            },
            ..sample("small", n, 1, 10)
        };
        let samples: Vec<_> = (1..=5).map(reset).collect();
        assert_eq!(
            estimate_reserve(&samples, &run("small", "sonnet"), BUFFER)
                .reserve
                .five_hour,
            10
        );
        let dropped = UsageSample {
            after: snapshot(5, 21),
            ..sample("small", 0, 1, 10)
        };
        assert_eq!(FIVE_HOUR(&dropped), None);
        assert_eq!(WEEK(&dropped), Some(1));
        let unknown = UsageSample {
            before: UsageSnapshot::default(),
            ..sample("small", 1, 1, 10)
        };
        assert_eq!(FIVE_HOUR(&unknown), None);
    }

    #[test]
    fn the_source_of_the_five_hour_window_is_reported() {
        let mut samples: Vec<_> = (0..5).map(|_| sample("small", 3, 1, 10)).collect();
        for sample in &mut samples {
            sample.before.five_hour_resets_at = None;
        }
        let estimate = estimate_reserve(&samples, &run("small", "sonnet"), BUFFER);
        assert_eq!(estimate.source, "buffer");
        assert_eq!(estimate.reserve.week, 1);
    }

    #[test]
    fn minutes_round_up_and_negative_durations_are_left_out() {
        let mut samples: Vec<_> = (0..5).map(|_| sample("small", 3, 1, 10)).collect();
        samples[0].ended_at = at(T0 + 61_000);
        samples[1].ended_at = at(T0 - 1000);
        let minutes = estimate_minutes(&[], &samples.iter().collect::<Vec<_>>());
        assert_eq!(minutes, None);
        samples.push(sample("small", 3, 1, 10));
        let minutes = estimate_minutes(&[], &samples.iter().collect::<Vec<_>>());
        assert_eq!(minutes, Some(10));
        assert_eq!(percentile_75(&[61]), 61);
    }

    fn work(n: u128, start: UsageSnapshot) -> RunningWork {
        RunningWork {
            account: Id::from_uuid(uuid::Uuid::from_u128(n)),
            profile: String::from("small"),
            model: None,
            effort: None,
            start,
        }
    }

    #[test]
    fn pending_is_the_reserve_minus_what_the_account_used_since_the_start() {
        let account = Id::from_uuid(uuid::Uuid::from_u128(1));
        let mut states = HashMap::new();
        states.insert(
            account,
            AccountState {
                usage: snapshot(14, 20),
                ..AccountState::default()
            },
        );
        let running = [work(1, snapshot(10, 20)), work(1, snapshot(12, 20))];
        let pending = pending_reserve(&running, &[], BUFFER, &states);
        // (10 - 4) + (10 - 2) and 2 + 2
        assert_eq!(
            pending[&account],
            AccountReserve {
                five_hour: 14,
                week: 4
            }
        );
    }

    #[test]
    fn pending_never_goes_below_zero_and_ignores_unknown_values() {
        let account = Id::from_uuid(uuid::Uuid::from_u128(1));
        let mut states = HashMap::new();
        states.insert(
            account,
            AccountState {
                usage: snapshot(60, 20),
                ..AccountState::default()
            },
        );
        let pending = pending_reserve(&[work(1, snapshot(10, 20))], &[], BUFFER, &states);
        assert_eq!(pending[&account].five_hour, 0);
        let pending = pending_reserve(&[work(1, UsageSnapshot::default())], &[], BUFFER, &states);
        assert_eq!(pending[&account], BUFFER);
        let other = Id::from_uuid(uuid::Uuid::from_u128(2));
        let pending = pending_reserve(&[work(2, snapshot(1, 1))], &[], BUFFER, &states);
        assert_eq!(pending[&other], BUFFER);
    }
}
