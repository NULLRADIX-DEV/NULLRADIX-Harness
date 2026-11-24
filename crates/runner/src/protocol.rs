//! Files a run directory holds; the supervisor writes them, the runner reads them.

use std::fs::{self, File, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use nullradix_app::runs::{OutputLine, OutputStream, RunReport};
use nullradix_domain::{RunEnd, RunOutcome};
use nullradix_os::process_alive;
use serde::{Deserialize, Serialize};

/// Bytes per entry of the output index: the offset of one line as little-endian u64.
pub(crate) const INDEX_ENTRY: u64 = 8;
/// How long a supervisor may take to show up before its run counts as lost.
const START_GRACE: Duration = Duration::from_secs(30);

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

    /// Offset of every output line, so readers seek instead of scanning.
    pub(crate) fn index(&self) -> PathBuf {
        self.dir.join("output.idx")
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

    /// Locked by the supervisor for its whole life; unlike a pid it cannot be reused.
    pub(crate) fn lock(&self) -> PathBuf {
        self.dir.join("supervisor.lock")
    }
}

/// Takes the supervisor lock; it only appears under its name once held.
pub(crate) fn hold_lock(files: &RunFiles) -> io::Result<File> {
    let staging = files.dir().join("supervisor.lock.tmp");
    let file = File::create(&staging)?;
    file.lock()?;
    fs::rename(&staging, files.lock())?;
    Ok(file)
}

/// Whether the supervisor of the run still runs.
pub(crate) fn supervisor_alive(files: &RunFiles) -> io::Result<bool> {
    match File::open(files.lock()) {
        Ok(file) => match file.try_lock_shared() {
            Ok(()) => Ok(false),
            Err(TryLockError::WouldBlock) => Ok(true),
            Err(TryLockError::Error(error)) => Err(error),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => starting(files),
        Err(error) => Err(error),
    }
}

/// A supervisor without its lock yet is starting while its fresh pid lives, or briefly without one.
fn starting(files: &RunFiles) -> io::Result<bool> {
    match fs::read_to_string(files.supervisor()) {
        Ok(pid) => Ok(pid.trim().parse::<u32>().is_ok_and(process_alive)),
        // the daemon may have died between starting the supervisor and noting its pid
        Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::metadata(files.spec()) {
            Ok(spec) => Ok(spec
                .modified()?
                .elapsed()
                .is_ok_and(|age| age < START_GRACE)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
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
    fn into_report(self) -> RunReport {
        match self.outcome.parse::<RunOutcome>() {
            Ok(outcome) => RunReport {
                outcome,
                end: RunEnd {
                    exit_code: self.exit_code,
                    error: self.error,
                },
            },
            Err(_) => unreadable(&format!("unknown outcome {}", self.outcome)),
        }
    }
}

/// A result that cannot be read still ends the run, as a failure that says why.
fn unreadable(reason: &str) -> RunReport {
    RunReport {
        outcome: RunOutcome::Failed,
        end: RunEnd {
            exit_code: None,
            error: Some(format!("the run result is unreadable: {reason}")),
        },
    }
}

/// The report of a finished run, or none while it runs.
pub(crate) fn read_report(files: &RunFiles) -> io::Result<Option<RunReport>> {
    match read_json::<ResultFile>(&files.result()) {
        Ok(file) => Ok(file.map(ResultFile::into_report)),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            Ok(Some(unreadable(&error.to_string())))
        }
        Err(error) => Err(error),
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

fn open_existing(path: &Path) -> io::Result<Option<File>> {
    match File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Output lines after `after`, at most `limit`; lines still being written are left for later.
pub(crate) fn read_output(files: &RunFiles, after: i64, limit: u32) -> io::Result<Vec<OutputLine>> {
    let after = u64::try_from(after).unwrap_or(0);
    let Some(mut index) = open_existing(&files.index())? else {
        return Ok(Vec::new());
    };
    let position = after.saturating_mul(INDEX_ENTRY);
    // an entry still being written counts as missing
    if position.saturating_add(INDEX_ENTRY) > index.metadata()?.len() {
        return Ok(Vec::new());
    }
    index.seek(SeekFrom::Start(position))?;
    let mut entry = [0; 8];
    index.read_exact(&mut entry)?;
    let Some(output) = open_existing(&files.output())? else {
        return Ok(Vec::new());
    };
    let mut reader = BufReader::new(output);
    reader.seek(SeekFrom::Start(u64::from_le_bytes(entry)))?;
    let capacity = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut lines = Vec::new();
    let mut buffer = Vec::new();
    while lines.len() < capacity {
        buffer.clear();
        if reader.read_until(b'\n', &mut buffer)? == 0 || buffer.last() != Some(&b'\n') {
            break;
        }
        let record: OutputRecord = serde_json::from_slice(&buffer)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
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

    /// Writes output the way the supervisor does, then a torn line without its index entry.
    fn write_lines(files: &RunFiles, count: i64) {
        let mut output = File::create(files.output()).unwrap();
        let mut index = File::create(files.index()).unwrap();
        let mut offset: u64 = 0;
        for seq in 1..=count {
            let mut line = serde_json::to_vec(&OutputRecord {
                seq,
                stream: String::from("stdout"),
                text: format!("line {seq}"),
            })
            .unwrap();
            line.push(b'\n');
            output.write_all(&line).unwrap();
            index.write_all(&offset.to_le_bytes()).unwrap();
            offset += u64::try_from(line.len()).unwrap();
        }
        output.write_all(br#"{"seq":99,"str"#).unwrap();
        index.write_all(&offset.to_le_bytes()[..3]).unwrap();
    }

    #[test]
    fn reads_output_after_a_sequence_number_with_a_limit() {
        let dir = tempfile::tempdir().unwrap();
        let files = RunFiles::at(dir.path());
        assert_eq!(read_output(&files, 0, 10).unwrap(), Vec::new());
        write_lines(&files, 4);
        let seqs = |after, limit| {
            read_output(&files, after, limit)
                .unwrap()
                .iter()
                .map(|line| line.seq)
                .collect::<Vec<_>>()
        };
        assert_eq!(seqs(1, 2), vec![2, 3]);
        assert_eq!(seqs(0, 100), vec![1, 2, 3, 4]);
        assert_eq!(seqs(-5, 1), vec![1]);
        assert_eq!(seqs(4, 10), Vec::<i64>::new());
        assert_eq!(seqs(i64::MAX, 10), Vec::<i64>::new());
    }

    #[test]
    fn a_torn_line_is_left_for_later() {
        let dir = tempfile::tempdir().unwrap();
        let files = RunFiles::at(dir.path());
        write_lines(&files, 2);
        assert_eq!(read_output(&files, 2, 10).unwrap(), Vec::new());
        let mut index = File::options().append(true).open(files.index()).unwrap();
        index.write_all(&[0; 5]).unwrap();
        // now the index points at the torn line, which has no newline yet
        assert_eq!(read_output(&files, 2, 10).unwrap(), Vec::new());
        assert_eq!(read_output(&files, 0, 10).unwrap().len(), 2);
    }

    #[test]
    fn unreadable_results_end_the_run_as_failed() {
        let dir = tempfile::tempdir().unwrap();
        let files = RunFiles::at(dir.path());
        assert_eq!(read_report(&files).unwrap(), None);
        fs::write(files.result(), b"{not json").unwrap();
        let report = read_report(&files).unwrap().unwrap();
        assert_eq!(report.outcome, RunOutcome::Failed);
        fs::write(files.result(), br#"{"outcome":"exploded"}"#).unwrap();
        let report = read_report(&files).unwrap().unwrap();
        assert!(report.end.error.unwrap().contains("exploded"));
    }

    #[test]
    fn the_lock_tells_whether_the_supervisor_lives() {
        let dir = tempfile::tempdir().unwrap();
        let files = RunFiles::at(dir.path());
        assert!(!supervisor_alive(&files).unwrap());
        fs::write(files.spec(), b"{}").unwrap();
        assert!(
            supervisor_alive(&files).unwrap(),
            "a fresh spec means starting"
        );
        fs::write(files.supervisor(), b"4294967295").unwrap();
        assert!(
            !supervisor_alive(&files).unwrap(),
            "a dead pid without lock is gone"
        );
        fs::write(files.supervisor(), std::process::id().to_string()).unwrap();
        assert!(supervisor_alive(&files).unwrap());
        let lock = hold_lock(&files).unwrap();
        assert!(supervisor_alive(&files).unwrap());
        drop(lock);
        assert!(!supervisor_alive(&files).unwrap());
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
        .into_report();
        assert_eq!(report.outcome, RunOutcome::TimedOut);
        assert_eq!(report.end.error.as_deref(), Some("took too long"));
    }
}
