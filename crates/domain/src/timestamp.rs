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

    /// Reads RFC 3339 such as `2026-11-25T18:00:00.123Z` or `...+01:00`; extra digits drop.
    #[must_use]
    pub fn parse_rfc3339(text: &str) -> Option<Self> {
        let (date, rest) = text.trim().split_once(['T', 't', ' '])?;
        let (clock, offset) = split_offset(rest)?;
        let mut parts = date.split('-');
        let (year, month, day) = (
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        );
        if parts.next().is_some() {
            return None;
        }
        let (whole, fraction) = clock.split_once('.').unwrap_or((clock, ""));
        let mut fields = whole.split(':');
        let (hour, minute, second): (i64, i64, i64) = (
            fields.next()?.parse().ok()?,
            fields.next()?.parse().ok()?,
            fields.next()?.parse().ok()?,
        );
        if fields.next().is_some() || !fraction.chars().all(|digit| digit.is_ascii_digit()) {
            return None;
        }
        let millis: i64 = format!("{fraction:0<3}")[..3].parse().ok()?;
        moment(year, month, day, hour, minute, second)?
            .checked_mul(1000)?
            .checked_add(millis)?
            .checked_sub(offset.checked_mul(1000)?)
            .map(Self)
    }

    /// Reads an HTTP date such as `Sun, 06 Nov 1994 08:49:37 GMT`.
    #[must_use]
    pub fn parse_http_date(text: &str) -> Option<Self> {
        let mut parts = text.split_whitespace();
        let weekday = parts.next()?;
        let (day, month, year, clock, zone) = (
            parts.next()?.parse().ok()?,
            month_number(parts.next()?)?,
            parts.next()?.parse().ok()?,
            parts.next()?,
            parts.next()?,
        );
        if !weekday.ends_with(',') || zone != "GMT" || parts.next().is_some() {
            return None;
        }
        let mut fields = clock.split(':');
        let (hour, minute, second) = (
            fields.next()?.parse().ok()?,
            fields.next()?.parse().ok()?,
            fields.next()?.parse().ok()?,
        );
        let seconds = moment(year, month, day, hour, minute, second)?;
        seconds.checked_mul(1000).map(Self)
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
pub(crate) const fn civil_date(days: i64) -> (i64, i64, i64) {
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

/// Splits `18:00:00.5+01:00` into the clock and the offset to UTC in seconds.
fn split_offset(rest: &str) -> Option<(&str, i64)> {
    if let Some(clock) = rest.strip_suffix(['Z', 'z']) {
        return Some((clock, 0));
    }
    let at = rest.rfind(['+', '-'])?;
    let (clock, sign_and_offset) = rest.split_at(at);
    let sign = if sign_and_offset.starts_with('-') {
        -1
    } else {
        1
    };
    let (hours, minutes) = sign_and_offset[1..].split_once(':')?;
    let (hours, minutes): (i64, i64) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours < 24 && minutes < 60).then_some((clock, sign * (hours * 3600 + minutes * 60)))
}

fn month_number(name: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let index = MONTHS
        .iter()
        .position(|month| month.eq_ignore_ascii_case(name))?;
    i64::try_from(index + 1).ok()
}

/// Seconds since the epoch of a valid calendar moment.
fn moment(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> Option<i64> {
    let valid = (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && (0..24).contains(&hour)
        && (0..60).contains(&minute)
        && (0..=60).contains(&second);
    if !valid {
        return None;
    }
    let days = days_from_civil(year, month, day);
    // a date that civil_date reads back differently, such as February 30th, is no date
    if civil_date(days) != (year, month, day) {
        return None;
    }
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Day count since 1970-01-01 of a calendar date, after Howard Hinnant's algorithm.
pub(crate) const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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

    #[test]
    fn reads_rfc_3339_in_any_offset() {
        let cases = [
            ("1970-01-01T00:00:00Z", Some(0)),
            ("2023-11-14T22:13:20.123Z", Some(1_700_000_000_123)),
            ("2023-11-14T23:13:20.123+01:00", Some(1_700_000_000_123)),
            ("2023-11-14T17:13:20.123456-05:00", Some(1_700_000_000_123)),
            ("2023-11-14t22:13:20z", Some(1_700_000_000_000)),
            ("2000-02-29T00:00:00Z", Some(951_782_400_000)),
            ("2023-11-14T22:13:20.5Z", Some(1_700_000_000_500)),
            ("1969-12-31T23:59:59.999Z", Some(-1)),
            ("2023-02-29T00:00:00Z", None),
            ("2023-13-01T00:00:00Z", None),
            ("2023-11-14T24:00:00Z", None),
            ("2023-11-14T22:13:20", None),
            ("2023-11-14", None),
            ("yesterday", None),
            ("2023-11-14T22:13:20.xZ", None),
        ];
        for (text, millis) in cases {
            assert_eq!(
                Timestamp::parse_rfc3339(text),
                millis.map(Timestamp::from_millis),
                "{text}"
            );
        }
    }

    #[test]
    fn rfc_3339_round_trips_with_the_printer() {
        for millis in [0, 1_700_000_000_123, 951_782_400_000, -1, 4_102_444_800_000] {
            let stamp = Timestamp::from_millis(millis);
            assert_eq!(Timestamp::parse_rfc3339(&stamp.to_string()), Some(stamp));
        }
    }

    #[test]
    fn reads_http_dates() {
        assert_eq!(
            Timestamp::parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(Timestamp::from_millis(784_111_777_000))
        );
        for bad in [
            "",
            "06 Nov 1994 08:49:37 GMT",
            "Sun, 06 Foo 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 08:49:37 PST",
            "120",
        ] {
            assert_eq!(Timestamp::parse_http_date(bad), None, "{bad}");
        }
    }
}
