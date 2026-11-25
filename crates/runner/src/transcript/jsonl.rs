//! Complete lines of a JSONL file with their byte offsets; a last line without newline waits.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

/// A complete line: start in the file, end behind its newline, and its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Line<'a> {
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) text: &'a str,
}

/// `base` moved on by `index` bytes.
pub(crate) fn offset(base: u64, index: usize) -> u64 {
    base.saturating_add(u64::try_from(index).unwrap_or(u64::MAX))
}

/// A count as the API reports it.
pub(crate) fn count(items: usize) -> i64 {
    i64::try_from(items).unwrap_or(i64::MAX)
}

/// Complete lines of `bytes` read from `base`; blank lines drop, bad UTF-8 counts as broken.
pub(crate) fn lines(bytes: &[u8], base: u64) -> Vec<Line<'_>> {
    let mut list = Vec::new();
    let mut start = 0usize;
    while let Some(found) = bytes[start..].iter().position(|byte| *byte == b'\n') {
        let end = start + found;
        let raw = &bytes[start..end];
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        let text = std::str::from_utf8(raw).unwrap_or("\u{FFFD}");
        let text = text.trim_start_matches('\u{FEFF}');
        if !text.trim().is_empty() {
            list.push(Line {
                start: offset(base, start),
                end: offset(base, end + 1),
                text,
            });
        }
        start = end + 1;
    }
    list
}

/// Offset behind the last complete line.
pub(crate) fn complete_end(bytes: &[u8], base: u64) -> u64 {
    match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => offset(base, index + 1),
        None => base,
    }
}

/// The file from `offset` to its end; an offset behind the end reads nothing.
pub(crate) fn read_from(path: &Path, offset: u64) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if offset >= len {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::with_capacity(usize::try_from(len - offset).unwrap_or(0));
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// A text field of an object; blank texts count as missing.
pub(crate) fn text<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
}

/// A flag of an object; missing is false.
pub(crate) fn flag(value: &Value, name: &str) -> bool {
    value.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// A number of an object as `i64`, floats rounded.
pub(crate) fn number(value: &Value, name: &str) -> Option<i64> {
    let field = value.get(name)?;
    field.as_i64().or_else(|| {
        // formatting a rounded float avoids a lossy cast
        let rounded = field.as_f64()?.round();
        format!("{rounded:.0}").parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_complete_lines_count_with_their_offsets() {
        let bytes = b"{\"a\":1}\r\n\n{\"b\":2}\n{\"half";
        let found = lines(bytes, 10);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].text, "{\"a\":1}");
        assert_eq!((found[0].start, found[0].end), (10, 19));
        assert_eq!(found[1].text, "{\"b\":2}");
        assert_eq!((found[1].start, found[1].end), (20, 28));
        assert_eq!(complete_end(bytes, 10), 28);
    }

    #[test]
    fn without_a_newline_nothing_is_complete() {
        assert_eq!(lines(b"{\"a\":1}", 0), Vec::new());
        assert_eq!(complete_end(b"{\"a\":1}", 5), 5);
    }

    #[test]
    fn numbers_round_floats() {
        let value = json!({ "a": 2.6, "b": 3, "c": "x" });
        assert_eq!(number(&value, "a"), Some(3));
        assert_eq!(number(&value, "b"), Some(3));
        assert_eq!(number(&value, "c"), None);
    }
}
