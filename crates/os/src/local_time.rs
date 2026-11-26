/// Minutes local time is ahead of UTC right now; `None` when the system does not say.
#[must_use]
pub fn local_utc_offset_minutes() -> Option<i32> {
    #[cfg(windows)]
    {
        crate::windows::utc_offset_minutes()
    }
    #[cfg(not(windows))]
    {
        unix_offset()
    }
}

/// Reads `+hhmm` or `-hhmm` as printed by `date +%z`.
#[cfg_attr(windows, allow(dead_code, reason = "only Unix asks the date command"))]
fn parse_offset(text: &str) -> Option<i32> {
    let text = text.trim();
    let sign = match text.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let digits = &text[1..];
    if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..].parse().ok()?;
    Some(sign * (hours * 60 + minutes))
}

#[cfg(not(windows))]
fn unix_offset() -> Option<i32> {
    let output = std::process::Command::new("date")
        .arg("+%z")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| parse_offset(&String::from_utf8_lossy(&output.stdout)))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_date_command_offset_is_read_in_minutes() {
        assert_eq!(parse_offset("+0100\n"), Some(60));
        assert_eq!(parse_offset("-0930"), Some(-570));
        assert_eq!(parse_offset("+0000"), Some(0));
        assert_eq!(parse_offset("0100"), None);
        assert_eq!(parse_offset("+01:00"), None);
        assert_eq!(parse_offset(""), None);
    }

    #[test]
    fn the_system_offset_is_a_plausible_one() {
        let offset = local_utc_offset_minutes().unwrap();
        assert!((-14 * 60..=14 * 60).contains(&offset));
    }
}
