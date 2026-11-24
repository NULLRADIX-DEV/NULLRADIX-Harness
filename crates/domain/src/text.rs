/// Length as the legacy board counted it: UTF-16 code units, like JavaScript.
#[must_use]
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The longest start of `text` within `units` UTF-16 code units, never splitting a character.
#[must_use]
pub fn utf16_prefix(text: &str, units: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|letter| {
            used += letter.len_utf16();
            used <= units
        })
        .collect()
}

/// Why a text field was refused; the entity error names the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TextError {
    #[error("is empty")]
    Empty,
    #[error("has more than {0} characters")]
    TooLong(usize),
}

/// `text` without surrounding whitespace, not empty and at most `max` UTF-16 units long.
pub fn required_text(text: &str, max: usize) -> Result<&str, TextError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        Err(TextError::Empty)
    } else if utf16_len(trimmed) > max {
        Err(TextError::TooLong(max))
    } else {
        Ok(trimmed)
    }
}

/// `text` without surrounding whitespace; blank text is none, and at most `max` UTF-16 units.
pub fn optional_text(text: &str, max: usize) -> Result<Option<&str>, TextError> {
    match required_text(text, max) {
        Ok(text) => Ok(Some(text)),
        Err(TextError::Empty) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_text_is_trimmed_and_counted_in_utf16_units() {
        assert_eq!(required_text("  Main ", 4), Ok("Main"));
        assert_eq!(required_text(" \t", 4), Err(TextError::Empty));
        assert_eq!(
            required_text(&"\u{1f600}".repeat(3), 5),
            Err(TextError::TooLong(5))
        );
        assert_eq!(utf16_len("\u{1f600}a"), 3);
    }

    #[test]
    fn optional_text_turns_blank_into_none() {
        assert_eq!(optional_text(" ", 3), Ok(None));
        assert_eq!(optional_text(" abc ", 3), Ok(Some("abc")));
        assert_eq!(optional_text("abcd", 3), Err(TextError::TooLong(3)));
    }
}
