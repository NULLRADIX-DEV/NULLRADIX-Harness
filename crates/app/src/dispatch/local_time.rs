//! The wall clock of the machine, which the dispatcher's time window is read in.

use async_trait::async_trait;
use nullradix_domain::Timestamp;

const MINUTE_MS: i64 = 60_000;
const DAY_MINUTES: i64 = 24 * 60;

/// Tells the local clock of the machine at a moment.
#[async_trait]
pub trait LocalTime: Send + Sync {
    /// Minutes the local clock is ahead of UTC at a moment.
    async fn offset_minutes(&self, at: Timestamp) -> i32;

    async fn minute_of_day(&self, at: Timestamp) -> u32 {
        minute_with_offset(at, self.offset_minutes(at).await)
    }
}

/// The minute of the day shifted by a fixed offset from UTC.
#[must_use]
pub fn minute_with_offset(at: Timestamp, offset_minutes: i32) -> u32 {
    let minutes = at.as_millis().div_euclid(MINUTE_MS) + i64::from(offset_minutes);
    u32::try_from(minutes.rem_euclid(DAY_MINUTES)).unwrap_or(0)
}

/// A day number as `YYYY-MM-DD`.
#[must_use]
pub fn day_text(day: i64) -> String {
    let shifted = day + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let day_of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day_of_month = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day_of_month:02}")
}

/// UTC as local time, for tests and machines whose offset is unknown.
#[derive(Debug, Clone, Copy, Default)]
pub struct UtcTime;

#[async_trait]
impl LocalTime for UtcTime {
    async fn offset_minutes(&self, _at: Timestamp) -> i32 {
        0
    }
}

#[cfg(test)]
mod tests {
    use nullradix_domain::local_day_of;

    use super::*;

    fn at(hours: i64, minutes: i64) -> Timestamp {
        Timestamp::from_millis((hours * 60 + minutes) * MINUTE_MS)
    }

    #[test]
    fn the_local_day_follows_the_offset() {
        // 2026-11-26 23:30 UTC
        let moment = Timestamp::from_millis(1_795_735_800_000);
        assert_eq!(day_text(local_day_of(moment, 0)), "2026-11-26");
        assert_eq!(day_text(local_day_of(moment, 60)), "2026-11-27");
        assert_eq!(day_text(local_day_of(moment, -300)), "2026-11-26");
        // UTC-10 and UTC+14 show the same clock but are a day apart
        assert_eq!(day_text(local_day_of(moment, -600)), "2026-11-26");
        assert_eq!(day_text(local_day_of(moment, 14 * 60)), "2026-11-27");
        assert_eq!(
            day_text(local_day_of(moment, -(23 * 60 + 31))),
            "2026-11-25"
        );
        assert_eq!(day_text(0), "1970-01-01");
        assert_eq!(day_text(11_016), "2000-02-29");
    }

    #[test]
    fn the_offset_moves_the_minute_across_midnight() {
        assert_eq!(minute_with_offset(at(10, 30), 0), 10 * 60 + 30);
        assert_eq!(minute_with_offset(at(23, 30), 60), 30);
        assert_eq!(minute_with_offset(at(0, 15), -60), 23 * 60 + 15);
        assert_eq!(
            minute_with_offset(Timestamp::from_millis(-MINUTE_MS), 0),
            23 * 60 + 59
        );
    }
}
