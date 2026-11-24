//! Runs one process to completion on behalf of the daemon, which may come and go meanwhile.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use nullradix_domain::RunOutcome;
use nullradix_os::{ProcessTree, write_atomic};

use crate::protocol::{OutputRecord, ResultFile, RunFiles, SpecFile, read_json};

const TICK: Duration = Duration::from_millis(100);
/// Grandchildren may hold the pipes open after the child exits; stop waiting after this.
const DRAIN: Duration = Duration::from_secs(10);

/// Executes the run described in `dir` and always leaves a `result.json` behind.
pub fn supervise(dir: &Path) -> io::Result<()> {
    let files = RunFiles::at(dir);
    let result = match read_json::<SpecFile>(&files.spec()) {
        Ok(Some(spec)) => execute(&files, &spec),
        Ok(None) => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "spec.json is missing",
        )),
        Err(error) => Err(error),
    };
    let report =
        result.unwrap_or_else(|error| failure(None, &format!("supervisor failed: {error}")));
    let json = serde_json::to_vec(&report).map_err(io::Error::other)?;
    write_atomic(&files.result(), &json)
}

fn failure(exit_code: Option<i32>, error: &str) -> ResultFile {
    ResultFile {
        outcome: RunOutcome::Failed.as_str().to_owned(),
        exit_code,
        error: Some(error.to_owned()),
    }
}

enum Ending {
    Exited(ExitStatus),
    Cancelled,
    TimedOut,
}

fn execute(files: &RunFiles, spec: &SpecFile) -> io::Result<ResultFile> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tree = match ProcessTree::spawn(&mut command) {
        Ok(tree) => tree,
        Err(error) => {
            return Ok(failure(
                Some(127),
                &format!("cannot start {}: {error}", spec.program),
            ));
        }
    };
    let (sender, lines) = mpsc::channel();
    if let Some(stdout) = tree.child_mut().stdout.take() {
        forward(stdout, "stdout", sender.clone());
    }
    if let Some(stderr) = tree.child_mut().stderr.take() {
        forward(stderr, "stderr", sender);
    }
    let mut output = OutputWriter::create(&files.output())?;
    let deadline = spec
        .timeout_ms
        .and_then(|millis| Instant::now().checked_add(Duration::from_millis(millis)));

    let ending = loop {
        match lines.recv_timeout(TICK) {
            Ok((stream, text)) => output.write(stream, &text)?,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
        }
        output.drain(&lines)?;
        if let Some(status) = tree.child_mut().try_wait()? {
            break Ending::Exited(status);
        }
        if files.cancel().exists() {
            tree.kill()?;
            tree.child_mut().wait()?;
            break Ending::Cancelled;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            tree.kill()?;
            tree.child_mut().wait()?;
            break Ending::TimedOut;
        }
    };
    output.finish(&lines)?;
    Ok(match ending {
        Ending::Exited(status) if status.success() => ResultFile {
            outcome: RunOutcome::Succeeded.as_str().to_owned(),
            exit_code: status.code(),
            error: None,
        },
        Ending::Exited(status) => ResultFile {
            outcome: RunOutcome::Failed.as_str().to_owned(),
            exit_code: status.code(),
            error: None,
        },
        Ending::Cancelled => ResultFile {
            outcome: RunOutcome::Cancelled.as_str().to_owned(),
            exit_code: None,
            error: None,
        },
        Ending::TimedOut => ResultFile {
            outcome: RunOutcome::TimedOut.as_str().to_owned(),
            exit_code: None,
            error: Some(String::from("the run exceeded its timeout")),
        },
    })
}

/// Sends complete lines of `pipe`; bytes are decoded per line so split characters survive.
fn forward(
    pipe: impl Read + Send + 'static,
    stream: &'static str,
    sender: Sender<(&'static str, String)>,
) {
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    while matches!(buffer.last(), Some(b'\n' | b'\r')) {
                        buffer.pop();
                    }
                    let text = String::from_utf8_lossy(&buffer).into_owned();
                    if sender.send((stream, text)).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

struct OutputWriter {
    file: BufWriter<File>,
    seq: i64,
}

impl OutputWriter {
    fn create(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file: BufWriter::new(file),
            seq: 0,
        })
    }

    fn write(&mut self, stream: &str, text: &str) -> io::Result<()> {
        self.seq += 1;
        let record = OutputRecord {
            seq: self.seq,
            stream: stream.to_owned(),
            text: text.to_owned(),
        };
        serde_json::to_writer(&mut self.file, &record).map_err(io::Error::other)?;
        self.file.write_all(b"\n")?;
        self.file.flush()
    }

    fn drain(&mut self, lines: &Receiver<(&'static str, String)>) -> io::Result<()> {
        while let Ok((stream, text)) = lines.try_recv() {
            self.write(stream, &text)?;
        }
        Ok(())
    }

    fn finish(&mut self, lines: &Receiver<(&'static str, String)>) -> io::Result<()> {
        let deadline = Instant::now() + DRAIN;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match lines.recv_timeout(left) {
                Ok((stream, text)) => self.write(stream, &text)?,
                Err(_) => return Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::protocol::read_output;

    fn shell(script: &str) -> (String, Vec<String>) {
        if cfg!(windows) {
            (
                String::from("cmd"),
                vec![String::from("/C"), script.to_owned()],
            )
        } else {
            (
                String::from("sh"),
                vec![String::from("-c"), script.to_owned()],
            )
        }
    }

    fn prepare(dir: &Path, script: &str, timeout_ms: Option<u64>) -> RunFiles {
        let (program, args) = shell(script);
        let spec = SpecFile {
            program,
            args,
            cwd: dir.to_path_buf(),
            timeout_ms,
        };
        fs::write(dir.join("spec.json"), serde_json::to_vec(&spec).unwrap()).unwrap();
        RunFiles::at(dir)
    }

    fn result(files: &RunFiles) -> ResultFile {
        read_json::<ResultFile>(&files.result()).unwrap().unwrap()
    }

    #[test]
    fn records_output_in_order_and_the_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let files = prepare(dir.path(), "echo first&& echo second&& exit 3", None);
        supervise(files.dir()).unwrap();
        let report = result(&files);
        assert_eq!(report.outcome, "failed");
        assert_eq!(report.exit_code, Some(3));
        let lines = read_output(&files.output(), 0, 10).unwrap();
        let texts: Vec<&str> = lines.iter().map(|line| line.text.trim()).collect();
        assert_eq!(texts, vec!["first", "second"]);
        assert_eq!(lines[1].seq, 2);
    }

    #[test]
    fn reports_success() {
        let dir = tempfile::tempdir().unwrap();
        let files = prepare(dir.path(), "echo done", None);
        supervise(files.dir()).unwrap();
        assert_eq!(result(&files).outcome, "succeeded");
        assert_eq!(result(&files).exit_code, Some(0));
    }

    #[test]
    fn a_missing_program_fails_with_127() {
        let dir = tempfile::tempdir().unwrap();
        let spec = SpecFile {
            program: String::from("nullradix-no-such-program"),
            args: Vec::new(),
            cwd: dir.path().to_path_buf(),
            timeout_ms: None,
        };
        fs::write(
            dir.path().join("spec.json"),
            serde_json::to_vec(&spec).unwrap(),
        )
        .unwrap();
        let files = RunFiles::at(dir.path());
        supervise(files.dir()).unwrap();
        let report = result(&files);
        assert_eq!(
            (report.outcome.as_str(), report.exit_code),
            ("failed", Some(127))
        );
    }

    #[test]
    fn a_missing_spec_still_leaves_a_result() {
        let dir = tempfile::tempdir().unwrap();
        let files = RunFiles::at(dir.path());
        supervise(files.dir()).unwrap();
        assert!(result(&files).error.unwrap().contains("spec.json"));
    }

    #[test]
    fn stops_at_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let script = if cfg!(windows) {
            "ping -n 30 127.0.0.1 > NUL"
        } else {
            "sleep 30"
        };
        let files = prepare(dir.path(), script, Some(300));
        let started = Instant::now();
        supervise(files.dir()).unwrap();
        assert!(started.elapsed() < Duration::from_secs(15));
        assert_eq!(result(&files).outcome, "timed-out");
    }

    #[test]
    fn stops_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let script = if cfg!(windows) {
            "ping -n 30 127.0.0.1 > NUL"
        } else {
            "sleep 30"
        };
        let files = prepare(dir.path(), script, None);
        let cancel = files.cancel();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            fs::write(cancel, b"").unwrap();
        });
        supervise(files.dir()).unwrap();
        canceller.join().unwrap();
        assert_eq!(result(&files).outcome, "cancelled");
    }
}
