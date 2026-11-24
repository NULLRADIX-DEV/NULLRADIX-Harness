//! Files a run directory holds; the supervisor writes them, the runner reads them.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use nullradix_app::runs::{OutputLine, OutputStream, RunReport};
use nullradix_domain::{RunEnd, RunOutcome};
use serde::{Deserialize, Serialize};

/// Paths inside the directory of one run.
#[derive(Debug, Clone)]
pub(crate) struct RunFiles {
    dir: PathBuf,
}

impl RunFiles {
    pub(crate) fn at(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn spec(&self) -> PathBuf {
        self.dir.join("spec.json")
    }

    pub(crate) fn output(&self) -> PathBuf {
        self.dir.join("output.jsonl")
    }

    pub(crate) fn result(&self) -> PathBuf {
        self.dir.join("result.json")
    }

    pub(crate) fn cancel(&self) -> PathBuf {
        self.dir.join("cancel")
    }

    pub(crate) fn supervisor(&self) -> PathBuf {
        self.dir.join("supervisor.pid")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SpecFile {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutputRecord {
    pub(crate) seq: i64,
    pub(crate) stream: String,
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResultFile {
    pub(crate) outcome: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) error: Option<String>,
}

impl ResultFile {
    pub(crate) fn into_report(self) -> io::Result<RunReport> {
        let outcome = self
            .outcome
            .parse::<RunOutcome>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "unknown run outcome"))?;
        Ok(RunReport {
            outcome,
            end: RunEnd {
                exit_code: self.exit_code,
                error: self.error,
            },
        })
    }
}

/// Reads a JSON file another process may still be renaming into place.
pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Option<T>> {
    const ATTEMPTS: u32 = 5;
    let mut attempt = 1;
    loop {
        match fs::read(path) {
            Ok(bytes) => {
                return serde_json::from_slice(&bytes)
                    .map(Some)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) if attempt < ATTEMPTS => {
                attempt += 1;
                drop(error);
                thread::sleep(Duration::from_millis(200));
            }
            Err(error) => return Err(error),
        }
    }
}

/// Output lines after `after`, at most `limit`; a torn last line is skipped until complete.
pub(crate) fn read_output(path: &Path, after: i64, limit: u32) -> io::Result<Vec<OutputLine>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let capacity = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut lines = Vec::new();
    for line in BufReader::new(file).lines() {
        if lines.len() >= capacity {
            break;
        }
        let Ok(record) = serde_json::from_str::<OutputRecord>(&line?) else {
            break;
        };
        if record.seq <= after {
            continue;
        }
        let stream = record
            .stream
            .parse::<OutputStream>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "unknown output stream"))?;
        lines.push(OutputLine {
            seq: record.seq,
            stream,
            text: record.text,
        });
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn record(seq: i64, text: &str) -> String {
        serde_json::to_string(&OutputRecord {
            seq,
            stream: String::from("stdout"),
            text: text.to_owned(),
        })
        .unwrap()
    }

    #[test]
    fn reads_output_after_a_sequence_number_with_a_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output.jsonl");
        let mut file = File::create(&path).unwrap();
        for seq in 1..=4 {
            writeln!(file, "{}", record(seq, &format!("line {seq}"))).unwrap();
        }
        write!(file, "{{\"seq\":5,\"str").unwrap();
        let lines = read_output(&path, 1, 2).unwrap();
        assert_eq!(
            lines.iter().map(|line| line.seq).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(read_output(&path, 0, 100).unwrap().len(), 4);
        assert_eq!(
            read_output(&dir.path().join("missing"), 0, 10).unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn missing_json_files_read_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let missing: Option<ResultFile> = read_json(&dir.path().join("result.json")).unwrap();
        assert_eq!(missing, None);
    }

    #[test]
    fn results_turn_into_reports() {
        let report = ResultFile {
            outcome: String::from("timed-out"),
            exit_code: None,
            error: Some(String::from("took too long")),
        }
        .into_report()
        .unwrap();
        assert_eq!(report.outcome, RunOutcome::TimedOut);
        assert_eq!(report.end.error.as_deref(), Some("took too long"));
    }
}
