//! The wall clock of the machine, which the dispatcher's time window is read in.

use async_trait::async_trait;
use nullradix_domain::Timestamp;

const MINUTE_MS: i64 = 60_000;
const DAY_MINUTES: i64 = 24 * 60;

/// Tells the local minute of the day at a moment.
#[async_trait]
pub trait LocalTime: Send + Sync {
    async fn minute_of_day(&self, at: Timestamp) -> u32;
}

/// The minute of the day shifted by a fixed offset from UTC.
#[must_use]
pub fn minute_with_offset(at: Timestamp, offset_minutes: i32) -> u32 {
    let minutes = at.as_millis().div_euclid(MINUTE_MS) + i64::from(offset_minutes);
    u32::try_from(minutes.rem_euclid(DAY_MINUTES)).unwrap_or(0)
}

/// UTC as local time, for tests and machines whose offset is unknown.
#[derive(Debug, Clone, Copy, Default)]
pub struct UtcTime;

#[async_trait]
impl LocalTime for UtcTime {
    async fn minute_of_day(&self, at: Timestamp) -> u32 {
        minute_with_offset(at, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hours: i64, minutes: i64) -> Timestamp {
        Timestamp::from_millis((hours * 60 + minutes) * MINUTE_MS)
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
