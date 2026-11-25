//! Complete lines of a JSONL file with their byte offsets; a last line without newline waits.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
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

/// Text of a raw line: no line break, bad UTF-8 counts as broken, a byte order mark drops.
fn line_text(raw: &[u8]) -> &str {
    let raw = raw.strip_suffix(b"\n").unwrap_or(raw);
    let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
    let text = std::str::from_utf8(raw).unwrap_or("\u{FFFD}");
    text.trim_start_matches('\u{FEFF}')
}

/// Complete lines of a source one at a time, so a page never holds more than one line.
pub(crate) struct LineReader<R> {
    source: R,
    /// Offset of the next unread byte.
    next: u64,
    /// Offset behind the last complete line, blank ones included.
    complete: u64,
    buffer: Vec<u8>,
}

impl<R: BufRead> LineReader<R> {
    /// Reads `source` that starts at offset `base` of its file.
    pub(crate) fn new(source: R, base: u64) -> Self {
        Self {
            source,
            next: base,
            complete: base,
            buffer: Vec::new(),
        }
    }

    /// The next complete line; blank lines drop and a last line without newline waits.
    pub(crate) fn next_line(&mut self) -> io::Result<Option<Line<'_>>> {
        let (start, end) = loop {
            self.buffer.clear();
            let length = self.source.read_until(b'\n', &mut self.buffer)?;
            if self.buffer.last() != Some(&b'\n') {
                return Ok(None);
            }
            let start = self.next;
            self.next = offset(start, length);
            self.complete = self.next;
            if !line_text(&self.buffer).trim().is_empty() {
                break (start, self.next);
            }
        };
        Ok(Some(Line {
            start,
            end,
            text: line_text(&self.buffer),
        }))
    }

    /// Offset behind the last complete line read.
    pub(crate) fn complete(&self) -> u64 {
        self.complete
    }
}

/// Lines of the file at `path` from `offset` on; an offset behind the end reads nothing.
pub(crate) fn open_lines(path: &Path, offset: u64) -> io::Result<LineReader<BufReader<File>>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    Ok(LineReader::new(BufReader::new(file), offset))
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
    use std::io::Cursor;

    use serde_json::json;

    use super::*;

    type Found = (Vec<(u64, u64, String)>, u64);

    fn read(bytes: &[u8], base: u64) -> Found {
        let mut reader = LineReader::new(Cursor::new(bytes), base);
        let mut found = Vec::new();
        while let Some(line) = reader.next_line().unwrap() {
            found.push((line.start, line.end, line.text.to_owned()));
        }
        (found, reader.complete())
    }

    #[test]
    fn only_complete_lines_count_with_their_offsets() {
        let bytes = b"{\"a\":1}\r\n\n{\"b\":2}\n{\"half";
        let (found, complete) = read(bytes, 10);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], (10, 19, String::from("{\"a\":1}")));
        assert_eq!(found[1], (20, 28, String::from("{\"b\":2}")));
        assert_eq!(complete, 28);
    }

    #[test]
    fn without_a_newline_nothing_is_complete() {
        assert_eq!(read(b"{\"a\":1}", 5), (Vec::new(), 5));
    }

    #[test]
    fn blank_lines_move_the_complete_end_and_bad_text_is_marked() {
        let (found, complete) = read(b"{}\n\n\xFF\xFE\n\xEF\xBB\xBF{}\n \n", 0);
        let texts: Vec<&str> = found.iter().map(|line| line.2.as_str()).collect();
        assert_eq!(texts, ["{}", "\u{FFFD}", "{}"]);
        assert_eq!(complete, 15);
    }

    #[test]
    fn a_file_is_read_from_an_offset_line_by_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        std::fs::write(&path, "{\"a\":1}\n{\"b\":2}\n").unwrap();
        let mut reader = open_lines(&path, 8).unwrap();
        assert_eq!(reader.next_line().unwrap().unwrap().text, "{\"b\":2}");
        assert!(reader.next_line().unwrap().is_none());
        assert_eq!(reader.complete(), 16);
        let mut behind = open_lines(&path, 99).unwrap();
        assert!(behind.next_line().unwrap().is_none());
        assert_eq!(behind.complete(), 99);
    }

    #[test]
    fn numbers_round_floats() {
        let value = json!({ "a": 2.6, "b": 3, "c": "x" });
        assert_eq!(number(&value, "a"), Some(3));
        assert_eq!(number(&value, "b"), Some(3));
        assert_eq!(number(&value, "c"), None);
    }
}
