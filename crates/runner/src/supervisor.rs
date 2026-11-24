//! Runs one process to completion on behalf of the daemon, which may come and go meanwhile.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use nullradix_domain::RunOutcome;
use nullradix_os::{ProcessTree, write_atomic};

use crate::protocol::{OutputRecord, ResultFile, RunFiles, SpecFile, hold_lock, read_json};

const TICK: Duration = Duration::from_millis(100);
/// Longer lines are split, so a child without newlines cannot exhaust memory.
const MAX_LINE: u64 = 64 * 1024;
/// Grandchildren may hold the pipes open after the child exits; stop waiting after this.
const DRAIN: Duration = Duration::from_secs(10);

/// Executes the run described in `dir` and always leaves a `result.json` behind.
pub fn supervise(dir: &Path) -> io::Result<()> {
    let files = RunFiles::at(dir);
    // held until the exit, which is how the runner learns that this process is gone
    let lock = hold_lock(&files);
    let result = match (&lock, read_json::<SpecFile>(&files.spec())) {
        (Err(error), _) => Err(io::Error::new(
            error.kind(),
            format!("cannot take the supervisor lock: {error}"),
        )),
        (Ok(_), Ok(Some(spec))) => execute(&files, &spec),
        (Ok(_), Ok(None)) => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "spec.json is missing",
        )),
        (Ok(_), Err(error)) => Err(error),
    };
    let report =
        result.unwrap_or_else(|error| failure(None, &format!("supervisor failed: {error}")));
    let json = serde_json::to_vec(&report).map_err(io::Error::other)?;
    write_atomic(&files.result(), &json)?;
    drop(lock);
    Ok(())
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

/// Relative paths such as `./build.sh` mean the run's directory, on Windows too.
fn program(spec: &SpecFile) -> PathBuf {
    let program = Path::new(&spec.program);
    if program.is_relative() && program.components().count() > 1 {
        spec.cwd.join(program)
    } else {
        program.to_path_buf()
    }
}

fn execute(files: &RunFiles, spec: &SpecFile) -> io::Result<ResultFile> {
    // a start the daemon gave up on is cancelled before the command can do anything
    if files.cancel().exists() {
        return Ok(Ending::Cancelled.into_result());
    }
    let mut command = Command::new(program(spec));
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
    if let Some(stdout) = tree.take_stdout() {
        forward(stdout, "stdout", sender.clone());
    }
    if let Some(stderr) = tree.take_stderr() {
        forward(stderr, "stderr", sender);
    }
    let mut output = OutputWriter::create(files)?;
    let deadline = spec
        .timeout_ms
        .and_then(|millis| Instant::now().checked_add(Duration::from_millis(millis)));

    let ending = loop {
        match lines.recv_timeout(TICK) {
            Ok((stream, text)) => output.write(stream, &text)?,
            Err(RecvTimeoutError::Timeout) => {}
            // both pipes closed while the child still runs
            Err(RecvTimeoutError::Disconnected) => thread::sleep(TICK),
        }
        output.drain(&lines)?;
        // the run ends with its main process, the tree kills what it left behind
        if let Some(status) = tree.try_wait()? {
            break Ending::Exited(status);
        }
        if files.cancel().exists() {
            tree.kill()?;
            tree.wait()?;
            break Ending::Cancelled;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            tree.kill()?;
            tree.wait()?;
            break Ending::TimedOut;
        }
    };
    output.finish(&lines)?;
    Ok(ending.into_result())
}

impl Ending {
    fn into_result(self) -> ResultFile {
        let (outcome, exit_code, error) = match self {
            Self::Exited(status) if status.success() => {
                (RunOutcome::Succeeded, status.code(), None)
            }
            Self::Exited(status) => (RunOutcome::Failed, status.code(), None),
            Self::Cancelled => (RunOutcome::Cancelled, None, None),
            Self::TimedOut => (
                RunOutcome::TimedOut,
                None,
                Some(String::from("the run exceeded its timeout")),
            ),
        };
        ResultFile {
            outcome: outcome.as_str().to_owned(),
            exit_code,
            error,
        }
    }
}

/// Sends the lines of `pipe` until it closes.
fn forward(
    pipe: impl Read + Send + 'static,
    stream: &'static str,
    sender: Sender<(&'static str, String)>,
) {
    thread::spawn(move || {
        let mut lines = Lines::new(pipe);
        while let Ok(Some(text)) = lines.next_line() {
            if sender.send((stream, text)).is_err() {
                break;
            }
        }
    });
}

/// Cuts a byte stream into lines of at most `MAX_LINE` bytes without breaking characters.
struct Lines<R> {
    reader: BufReader<R>,
    // the start of a character cut off at the end of the previous part
    carry: Vec<u8>,
    // whether the previous part ended without its line ending
    split: bool,
}

impl<R: Read> Lines<R> {
    fn new(pipe: R) -> Self {
        Self {
            reader: BufReader::new(pipe),
            carry: Vec::new(),
            split: false,
        }
    }

    /// The next line without its line ending, or `None` once the stream ended.
    fn next_line(&mut self) -> io::Result<Option<String>> {
        loop {
            let mut buffer = std::mem::take(&mut self.carry);
            let read = (&mut self.reader)
                .take(MAX_LINE)
                .read_until(b'\n', &mut buffer)?;
            if read == 0 {
                return Ok(
                    (!buffer.is_empty()).then(|| String::from_utf8_lossy(&buffer).into_owned())
                );
            }
            if buffer.last() == Some(&b'\n') {
                // only the line ending of a line already sent in parts
                if self.split && buffer == b"\n" {
                    self.split = false;
                    continue;
                }
                self.split = false;
                buffer.pop();
            } else {
                if let Err(error) = std::str::from_utf8(&buffer)
                    && error.error_len().is_none()
                {
                    self.carry = buffer.split_off(error.valid_up_to());
                }
                self.split = true;
            }
            if buffer.last() == Some(&b'\r') {
                buffer.pop();
            }
            return Ok(Some(String::from_utf8_lossy(&buffer).into_owned()));
        }
    }
}

/// Appends output lines and their offsets; a reader trusts a line once it has an index entry.
struct OutputWriter {
    output: File,
    index: File,
    offset: u64,
    seq: i64,
}

impl OutputWriter {
    fn create(files: &RunFiles) -> io::Result<Self> {
        Ok(Self {
            output: File::create(files.output())?,
            index: File::create(files.index())?,
            offset: 0,
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
        let mut line = serde_json::to_vec(&record).map_err(io::Error::other)?;
        line.push(b'\n');
        self.output.write_all(&line)?;
        self.index.write_all(&self.offset.to_le_bytes())?;
        self.offset += u64::try_from(line.len()).map_err(io::Error::other)?;
        Ok(())
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
        let lines = read_output(&files, 0, 10).unwrap();
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

    fn all_lines(bytes: &[u8]) -> Vec<String> {
        let mut lines = Lines::new(bytes);
        let mut all = Vec::new();
        while let Some(line) = lines.next_line().unwrap() {
            all.push(line);
        }
        all
    }

    #[test]
    fn cuts_lines_at_any_line_ending() {
        assert_eq!(all_lines(b"a\r\nb\n\nc"), vec!["a", "b", "", "c"]);
    }

    #[test]
    fn splits_long_lines_without_breaking_characters_or_line_endings() {
        let limit = usize::try_from(MAX_LINE).unwrap();
        let mut long = vec![b'x'; limit + 10];
        long.push(b'\n');
        let lengths: Vec<usize> = all_lines(&long).iter().map(String::len).collect();
        assert_eq!(lengths, vec![limit, 10]);

        let mut accent = vec![b'x'; limit - 1];
        accent.extend_from_slice("\u{e9}!\n".as_bytes());
        let lines = all_lines(&accent);
        assert_eq!(lines[0].len(), limit - 1);
        assert_eq!(lines[1], "\u{e9}!");

        let mut crlf = vec![b'x'; limit - 1];
        crlf.extend_from_slice(b"\r\nnext\n");
        let lines = all_lines(&crlf);
        assert_eq!((lines.len(), lines[1].as_str()), (2, "next"));
    }

    #[test]
    fn a_cancelled_start_never_runs_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let files = prepare(
            dir.path(),
            &format!("echo ran> \"{}\"", marker.display()),
            None,
        );
        fs::write(files.cancel(), b"").unwrap();
        supervise(files.dir()).unwrap();
        assert_eq!(result(&files).outcome, "cancelled");
        assert!(!marker.exists());
    }

    #[test]
    fn splits_overlong_lines() {
        let dir = tempfile::tempdir().unwrap();
        let (program, mut args) = shell("");
        args.pop();
        let script = if cfg!(windows) {
            "powershell -NoProfile -NonInteractive -Command [Console]::Out.Write('x' * 70000)"
        } else {
            "head -c 70000 /dev/zero | tr '\\0' x"
        };
        args.push(script.to_owned());
        let spec = SpecFile {
            program,
            args,
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
        let lengths: Vec<usize> = read_output(&files, 0, 10)
            .unwrap()
            .iter()
            .filter(|line| line.stream == nullradix_app::runs::OutputStream::Stdout)
            .map(|line| line.text.len())
            .collect();
        assert_eq!(lengths, vec![65536, 70000 - 65536]);
    }

    #[test]
    fn runs_relative_programs_from_the_run_directory() {
        let dir = tempfile::tempdir().unwrap();
        let (name, body) = if cfg!(windows) {
            ("hello.cmd", "@echo hello from the script\r\n")
        } else {
            ("hello.sh", "#!/bin/sh\necho hello from the script\n")
        };
        fs::write(dir.path().join(name), body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path().join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let spec = SpecFile {
            program: format!(".{}{name}", std::path::MAIN_SEPARATOR),
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
        assert_eq!(result(&files).outcome, "succeeded");
        let lines = read_output(&files, 0, 10).unwrap();
        assert_eq!(lines[0].text.trim(), "hello from the script");
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
