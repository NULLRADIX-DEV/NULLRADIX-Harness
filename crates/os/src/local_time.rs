use jiff::Timestamp;
use jiff::tz::TimeZone;

/// Minutes local time is ahead of UTC at a moment (Unix milliseconds); `None` when unknown.
#[must_use]
pub fn local_utc_offset_minutes(at_millis: i64) -> Option<i32> {
    let moment = Timestamp::from_millisecond(at_millis).ok()?;
    Some(TimeZone::system().to_offset(moment).seconds() / 60)
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
    #[test]
    fn the_offset_follows_daylight_saving_at_each_moment() {
        const CHILD: &str = "NULLRADIX_TZ_CHILD";
        if std::env::var_os(CHILD).is_some() {
            assert_eq!(local_utc_offset_minutes(JAN_15_2027), Some(60));
            assert_eq!(local_utc_offset_minutes(JUL_7_2027), Some(120));
            return;
        }
        let exe = std::env::current_exe().unwrap();
        let status = std::process::Command::new(exe)
            .args([
                "--exact",
                "local_time::tests::the_offset_follows_daylight_saving_at_each_moment",
            ])
            .env(CHILD, "1")
            .env("TZ", "CET-1CEST,M3.5.0,M10.5.0/3")
            .status()
            .unwrap();
        assert!(status.success());
    }
}
