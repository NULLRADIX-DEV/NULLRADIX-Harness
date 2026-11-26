#![allow(unsafe_code, reason = "libc time conversion without a safe wrapper")]

use nix::libc;

unsafe extern "C" {
    /// Re-reads the zone of the process, which libc does lazily and only once.
    safe fn tzset();
}

/// Minutes local time is ahead of UTC at a moment, by the zone rules of that moment.
pub(crate) fn utc_offset_minutes(at_millis: i64) -> Option<i32> {
    let seconds = libc::time_t::try_from(at_millis.div_euclid(1000)).ok()?;
    // SAFETY: the structure is plain data that the call fills in.
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    tzset();
    // SAFETY: both pointers are valid for the call.
    let done = unsafe { libc::localtime_r(&raw const seconds, &raw mut parts) };
    if done.is_null() {
        return None;
    }
    i32::try_from(i64::from(parts.tm_gmtoff).div_euclid(60)).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "tests fail loudly")]
mod tests {
    use super::*;

    const JAN_15_2027: i64 = 1_800_000_000_000;
    const JUL_7_2027: i64 = 1_815_000_000_000;

    #[test]
    fn the_offset_follows_daylight_saving_at_each_moment() {
        // SAFETY: a POSIX rule needs no zone files and nextest runs this test alone.
        unsafe { std::env::set_var("TZ", "CET-1CEST,M3.5.0,M10.5.0/3") };
        assert_eq!(utc_offset_minutes(JAN_15_2027), Some(60));
        assert_eq!(utc_offset_minutes(JUL_7_2027), Some(120));
        assert_eq!(utc_offset_minutes(JAN_15_2027), Some(60));
    }
}
