use std::fmt;
use std::time::Duration;

/// Milliseconds since the Unix epoch in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    #[must_use]
    pub const fn as_millis(self) -> i64 {
        self.0
    }

    #[must_use]
    pub fn checked_add(self, duration: Duration) -> Option<Self> {
        let millis = i64::try_from(duration.as_millis()).ok()?;
        self.0.checked_add(millis).map(Self)
    }

    /// Time elapsed since `earlier`, or zero when `earlier` lies in the future.
    #[must_use]
    pub fn saturating_since(self, earlier: Self) -> Duration {
        u64::try_from(self.0.saturating_sub(earlier.0))
            .map_or(Duration::ZERO, Duration::from_millis)
    }
}

/// RFC 3339 in UTC with milliseconds, such as `2026-11-24T22:15:03.120Z`.
impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seconds = self.0.div_euclid(1000);
        let second = seconds.rem_euclid(86_400);
        let (year, month, day) = civil_date(seconds.div_euclid(86_400));
        write!(
            f,
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
            second / 3600,
            second % 3600 / 60,
            second % 60,
            self.0.rem_euclid(1000)
        )
    }
}

/// Year, month and day of a day count since 1970-01-01, after Howard Hinnant's algorithm.
const fn civil_date(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_durations() {
        let start = Timestamp::from_millis(1_000);
        assert_eq!(
            start.checked_add(Duration::from_secs(2)),
            Some(Timestamp::from_millis(3_000))
        );
        assert_eq!(
            Timestamp::from_millis(i64::MAX).checked_add(Duration::from_millis(1)),
            None
        );
    }

    #[test]
    fn measures_elapsed_time() {
        let earlier = Timestamp::from_millis(1_000);
        let later = Timestamp::from_millis(4_500);
        assert_eq!(
            later.saturating_since(earlier),
            Duration::from_millis(3_500)
        );
        assert_eq!(earlier.saturating_since(later), Duration::ZERO);
    }

    #[test]
    fn prints_rfc_3339_in_utc() {
        let cases = [
            (0, "1970-01-01T00:00:00.000Z"),
            (1_700_000_000_123, "2023-11-14T22:13:20.123Z"),
            (951_782_400_000, "2000-02-29T00:00:00.000Z"),
            (-1, "1969-12-31T23:59:59.999Z"),
        ];
        for (millis, text) in cases {
            assert_eq!(Timestamp::from_millis(millis).to_string(), text);
        }
    }
}
