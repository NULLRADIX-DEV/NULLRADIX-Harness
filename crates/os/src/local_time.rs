use jiff::Timestamp;
use jiff::tz::TimeZone;

/// The time zone of the machine, read once and asked for many moments.
#[derive(Debug, Clone)]
pub struct LocalZone(TimeZone);

impl LocalZone {
    /// The system zone; `None` when the machine does not say which it is.
    #[must_use]
    pub fn system() -> Option<Self> {
        TimeZone::try_system().ok().map(Self)
    }

    /// Minutes local time is ahead of UTC at a moment (Unix milliseconds).
    #[must_use]
    pub fn offset_minutes(&self, at_millis: i64) -> Option<i32> {
        let moment = Timestamp::from_millisecond(at_millis).ok()?;
        Some(self.0.to_offset(moment).seconds() / 60)
    }
}

/// Minutes local time is ahead of UTC at a moment; `None` when the zone or moment is unknown.
#[must_use]
pub fn local_utc_offset_minutes(at_millis: i64) -> Option<i32> {
    LocalZone::system()?.offset_minutes(at_millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JAN_15_2027: i64 = 1_800_000_000_000;
    const JUL_7_2027: i64 = 1_815_000_000_000;

    #[test]
    fn the_offset_at_any_moment_is_a_plausible_one() {
        for at in [JAN_15_2027, JUL_7_2027, 0] {
            let offset = local_utc_offset_minutes(at).unwrap();
            assert!((-14 * 60..=14 * 60).contains(&offset), "{at}: {offset}");
        }
    }

    // The test runs itself with a zone set, so no test mutates its own environment.
    #[cfg(unix)]
    fn in_child(name: &str, zone: &str) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env("NULLRADIX_TZ_CHILD", "1")
            .env("TZ", zone)
            .output()
            .unwrap();
        let printed = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{printed}");
        assert!(
            printed.contains("running 1 test"),
            "the child ran no test: {printed}"
        );
        assert!(printed.contains("1 passed"), "{printed}");
    }

    #[cfg(unix)]
    fn check_berlin() {
        assert_eq!(local_utc_offset_minutes(JAN_15_2027), Some(60));
        assert_eq!(local_utc_offset_minutes(JUL_7_2027), Some(120));
    }

    #[cfg(unix)]
    #[test]
    fn the_offset_follows_daylight_saving_at_each_moment() {
        if std::env::var_os("NULLRADIX_TZ_CHILD").is_some() {
            return check_berlin();
        }
        in_child(
            "local_time::tests::the_offset_follows_daylight_saving_at_each_moment",
            "CET-1CEST,M3.5.0,M10.5.0/3",
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_zone_name_is_found_in_the_zone_files() {
        assert!(
            std::path::Path::new("/usr/share/zoneinfo").exists(),
            "install the tzdata package: the zone files are missing"
        );
        if std::env::var_os("NULLRADIX_TZ_CHILD").is_some() {
            return check_berlin();
        }
        in_child(
            "local_time::tests::a_zone_name_is_found_in_the_zone_files",
            "Europe/Berlin",
        );
    }
}
