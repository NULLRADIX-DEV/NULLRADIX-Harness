use crate::TimeWindow;

/// Minutes since midnight of an `HH:MM` text.
#[must_use]
pub fn minutes_of_day(text: &str) -> Option<u32> {
    let (hours, minutes) = text.split_once(':')?;
    let hours: u32 = hours.parse().ok()?;
    let minutes: u32 = minutes.parse().ok()?;
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

impl TimeWindow {
    /// Whether a run may start at this local minute of the day; equal ends mean always.
    #[must_use]
    pub fn contains(&self, minute: u32) -> bool {
        let (Some(from), Some(to)) = (minutes_of_day(&self.from), minutes_of_day(&self.to)) else {
            return true;
        };
        match from.cmp(&to) {
            std::cmp::Ordering::Equal => true,
            std::cmp::Ordering::Less => (from..to).contains(&minute),
            std::cmp::Ordering::Greater => minute >= from || minute < to,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(from: &str, to: &str) -> TimeWindow {
        TimeWindow {
            from: from.to_owned(),
            to: to.to_owned(),
        }
    }

    #[test]
    fn reads_hours_and_minutes_only() {
        assert_eq!(minutes_of_day("08:30"), Some(510));
        assert_eq!(minutes_of_day("24:00"), None);
        assert_eq!(minutes_of_day("12:60"), None);
        assert_eq!(minutes_of_day("noon"), None);
    }

    #[test]
    fn the_end_is_exclusive() {
        let day = window("08:00", "18:00");
        assert!(day.contains(8 * 60));
        assert!(day.contains(17 * 60 + 59));
        assert!(!day.contains(18 * 60));
        assert!(!day.contains(7 * 60 + 59));
    }

    #[test]
    fn a_window_past_midnight_wraps() {
        let night = window("22:00", "06:00");
        assert!(night.contains(23 * 60));
        assert!(night.contains(5 * 60 + 59));
        assert!(!night.contains(6 * 60));
        assert!(!night.contains(12 * 60));
    }

    #[test]
    fn equal_ends_mean_always() {
        assert!(window("09:00", "09:00").contains(3 * 60));
    }
}
