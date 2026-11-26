/// Minutes local time is ahead of UTC at a moment (Unix milliseconds); `None` when unknown.
#[must_use]
pub fn local_utc_offset_minutes(at_millis: i64) -> Option<i32> {
    #[cfg(windows)]
    {
        crate::windows::utc_offset_minutes(at_millis)
    }
    #[cfg(not(windows))]
    {
        crate::unix_time::utc_offset_minutes(at_millis)
    }
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
}
