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
}
