//! Files a run directory holds; the supervisor writes them, the runner reads them.

use std::collections::BTreeMap;
use std::fs::{self, File, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use nullradix_app::dispatch::stream::ClaudeOutput;
use nullradix_app::runs::{ClaudeReport, OutputLine, OutputStream, RunReport};
use nullradix_domain::{DeniedTool, ModelUsage, RunEnd, RunOutcome, RunUsage, Screenshot};
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

    /// Screenshots of a Claude run.
    pub(crate) fn artifacts(&self) -> PathBuf {
        self.dir.join("artifacts")
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

/// Environment changes of a run, as `RunEnv` of the daemon.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EnvFile {
    #[serde(default)]
    pub(crate) set: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) remove: Vec<String>,
    #[serde(default)]
    pub(crate) defaults: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SpecFile {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) timeout_ms: Option<u64>,
    #[serde(default)]
    pub(crate) env: EnvFile,
    #[serde(default)]
    pub(crate) stdin: Option<String>,
    #[serde(default)]
    pub(crate) claude: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelFile {
    pub(crate) model: String,
    pub(crate) input_tokens: Option<i64>,
    pub(crate) output_tokens: Option<i64>,
    pub(crate) cache_read_tokens: Option<i64>,
    pub(crate) cache_write_tokens: Option<i64>,
    pub(crate) cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageFile {
    pub(crate) cost_usd: Option<f64>,
    pub(crate) input_tokens: Option<i64>,
    pub(crate) output_tokens: Option<i64>,
    pub(crate) cache_read_tokens: Option<i64>,
    pub(crate) cache_write_tokens: Option<i64>,
    pub(crate) num_turns: Option<i64>,
    pub(crate) models: Vec<ModelFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScreenshotFile {
    pub(crate) tool_use_id: Option<String>,
    pub(crate) media_type: String,
    pub(crate) file: String,
    pub(crate) bytes: u64,
}

/// What the supervisor read from the output of a Claude run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeFile {
    pub(crate) session_id: Option<String>,
    pub(crate) has_result: bool,
    pub(crate) subtype: Option<String>,
    pub(crate) is_error: bool,
    pub(crate) error_text: String,
    pub(crate) usage: Option<UsageFile>,
    pub(crate) denied: Vec<(String, u32)>,
    pub(crate) denied_examples: Vec<String>,
    pub(crate) screenshots: Vec<ScreenshotFile>,
}

impl ClaudeFile {
    pub(crate) fn new(output: ClaudeOutput, screenshots: &[Screenshot]) -> Self {
        Self {
            session_id: output.session_id,
            has_result: output.has_result,
            subtype: output.subtype,
            is_error: output.is_error,
            error_text: output.error_text,
            usage: output.usage.map(|usage| UsageFile {
                cost_usd: usage.cost_usd,
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_write_tokens: usage.cache_write_tokens,
                num_turns: usage.num_turns,
                models: usage
                    .models
                    .into_iter()
                    .map(|model| ModelFile {
                        model: model.model,
                        input_tokens: model.input_tokens,
                        output_tokens: model.output_tokens,
                        cache_read_tokens: model.cache_read_tokens,
                        cache_write_tokens: model.cache_write_tokens,
                        cost_usd: model.cost_usd,
                    })
                    .collect(),
            }),
            denied: output
                .denied
                .into_iter()
                .map(|tool| (tool.tool, tool.count))
                .collect(),
            denied_examples: output.denied_examples,
            screenshots: screenshots
                .iter()
                .map(|shot| ScreenshotFile {
                    tool_use_id: shot.tool_use_id.clone(),
                    media_type: shot.media_type.clone(),
                    file: shot.file.clone(),
                    bytes: shot.bytes,
                })
                .collect(),
        }
    }

    fn into_report(self) -> ClaudeReport {
        let usage = self.usage.map(|usage| {
            // the main model is the one that cost the most, first on a tie
            let mut main: Option<&ModelFile> = None;
            for model in &usage.models {
                if main.is_none_or(|best| {
                    model.cost_usd.unwrap_or(-1.0) > best.cost_usd.unwrap_or(-1.0)
                }) {
                    main = Some(model);
                }
            }
            RunUsage {
                model: main.map(|model| model.model.clone()),
                cost_usd: usage.cost_usd,
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_write_tokens: usage.cache_write_tokens,
                num_turns: usage.num_turns,
                models: usage
                    .models
                    .into_iter()
                    .map(|model| ModelUsage {
                        model: model.model,
                        input_tokens: model.input_tokens,
                        output_tokens: model.output_tokens,
                        cache_read_tokens: model.cache_read_tokens,
                        cache_write_tokens: model.cache_write_tokens,
                        cost_usd: model.cost_usd,
                    })
                    .collect(),
            }
        });
        ClaudeReport {
            output: ClaudeOutput {
                session_id: self.session_id,
                has_result: self.has_result,
                subtype: self.subtype,
                is_error: self.is_error,
                error_text: self.error_text,
                usage,
                denied: self
                    .denied
                    .into_iter()
                    .map(|(tool, count)| DeniedTool { tool, count })
                    .collect(),
                denied_examples: self.denied_examples,
            },
            screenshots: self
                .screenshots
                .into_iter()
                .map(|shot| Screenshot {
                    tool_use_id: shot.tool_use_id,
                    media_type: shot.media_type,
                    file: shot.file,
                    bytes: shot.bytes,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutputRecord {
    pub(crate) seq: i64,
    pub(crate) stream: String,
    pub(crate) text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) continues: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResultFile {
    pub(crate) outcome: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) error: Option<String>,
    #[serde(default)]
    pub(crate) claude: Option<ClaudeFile>,
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
                claude: self.claude.map(ClaudeFile::into_report),
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
        claude: None,
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
            continues: record.continues,
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
                continues: false,
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
            claude: None,
        }
        .into_report();
        assert_eq!(report.outcome, RunOutcome::TimedOut);
        assert_eq!(report.end.error.as_deref(), Some("took too long"));
    }
}
