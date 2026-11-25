//! Runs one process to completion on behalf of the daemon, which may come and go meanwhile.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use nullradix_app::dispatch::stream::{
    ClaudeStream, DROPPED_LINE, Joined, LineJoiner, Pipe, images,
};
use nullradix_domain::{RunOutcome, Screenshot};
use nullradix_os::{ProcessTree, write_atomic};

use crate::launch::{self, Launch};
use crate::protocol::{
    ClaudeFile, EnvFile, OutputRecord, ResultFile, RunFiles, SpecFile, hold_lock, read_json,
};

const TICK: Duration = Duration::from_millis(100);
/// Longer lines are split, so a child without newlines cannot exhaust memory.
const MAX_LINE: u64 = 64 * 1024;
/// Grandchildren may hold the pipes open after the child exits; stop waiting after this.
const DRAIN: Duration = Duration::from_secs(10);
/// Screenshots kept per run, and the size of one.
const MAX_SCREENSHOTS: usize = 100;
const MAX_SCREENSHOT_BYTES: usize = 20 * 1024 * 1024;

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
        claude: None,
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

/// The command to start; on Windows a Claude run finds its program like the shell does.
fn command(spec: &SpecFile) -> Result<Command, launch::LaunchError> {
    #[cfg(windows)]
    if spec.claude {
        use std::os::windows::process::CommandExt;

        let path = ["PATH", "Path"]
            .iter()
            .find_map(|name| std::env::var(name).ok())
            .unwrap_or_default();
        let program = program(spec).to_string_lossy().into_owned();
        let (resolved, how) = launch::plan(&program, &spec.args, &path, &launch::file_exists)?;
        return Ok(match how {
            Launch::Direct => {
                let mut command = Command::new(resolved);
                command.args(&spec.args);
                command
            }
            Launch::Batch(line) => {
                let mut command = Command::new("cmd");
                command
                    .args(["/d", "/s", "/c"])
                    .raw_arg(format!("\"{line}\""));
                command
            }
        });
    }
    let mut command = Command::new(program(spec));
    command.args(&spec.args);
    Ok(command)
}

/// Applies the environment changes of the spec to what the supervisor itself inherited.
fn apply_env(command: &mut Command, env: &EnvFile, inherited: &dyn Fn(&str) -> Option<OsString>) {
    for name in &env.remove {
        command.env_remove(name);
    }
    for (name, default) in &env.defaults {
        if !env.set.contains_key(name) && inherited(name).is_none() {
            command.env(name, default);
        }
    }
    for (name, value) in &env.set {
        command.env(name, value);
    }
}

/// On Windows Claude Code needs Git Bash for the Bash rules to apply.
fn git_bash_env(spec: &SpecFile) -> EnvFile {
    let mut env = spec.env.clone();
    if cfg!(windows) && spec.claude && !env.set.contains_key("CLAUDE_CODE_GIT_BASH_PATH") {
        let lookup = |name: &str| std::env::var(name).ok();
        let git = launch::where_git();
        if let Some(found) = launch::git_bash(&lookup, git.as_deref(), &launch::file_exists) {
            env.set.insert(
                String::from("CLAUDE_CODE_USE_POWERSHELL_TOOL"),
                String::from("0"),
            );
            env.set
                .insert(String::from("CLAUDE_CODE_GIT_BASH_PATH"), found);
        }
    }
    env
}

/// A plain `claude` may be replaced by the command in `NULLRADIX_CLAUDE`, as for logins.
fn claude_override(spec: &SpecFile, replacement: Option<String>) -> SpecFile {
    let replacement = replacement.filter(|command| !command.trim().is_empty());
    match replacement {
        Some(command) if spec.claude && spec.program == "claude" => SpecFile {
            program: command,
            ..spec.clone()
        },
        _ => spec.clone(),
    }
}

fn execute(files: &RunFiles, spec: &SpecFile) -> io::Result<ResultFile> {
    let spec = &claude_override(spec, std::env::var("NULLRADIX_CLAUDE").ok());
    let with_claude = |file: ResultFile| ResultFile {
        claude: spec.claude.then(ClaudeFile::default),
        ..file
    };
    // a start the daemon gave up on is cancelled before the command can do anything
    if files.cancel().exists() {
        return Ok(with_claude(Ending::Cancelled.into_result()));
    }
    let mut command = match command(spec) {
        Ok(command) => command,
        Err(error) => return Ok(with_claude(failure(Some(127), &error.to_string()))),
    };
    command
        .current_dir(&spec.cwd)
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_env(&mut command, &git_bash_env(spec), &|name| {
        std::env::var_os(name)
    });
    let mut tree = match ProcessTree::spawn(&mut command) {
        Ok(tree) => tree,
        Err(error) => {
            // the same words on every platform, where the system message differs
            let reason = match error.kind() {
                io::ErrorKind::NotFound => String::from("program not found"),
                io::ErrorKind::PermissionDenied => String::from("permission denied"),
                _ => error.to_string(),
            };
            return Ok(with_claude(failure(
                Some(127),
                &format!("cannot start {}: {reason}", spec.program),
            )));
        }
    };
    if let (Some(mut pipe), Some(text)) = (tree.take_stdin(), spec.stdin.clone()) {
        // a thread, so a long prompt cannot block while the child is still starting
        thread::spawn(move || {
            let _ = pipe.write_all(text.as_bytes());
        });
    }
    let (sender, lines) = mpsc::channel();
    if let Some(stdout) = tree.take_stdout() {
        forward(stdout, "stdout", sender.clone());
    }
    if let Some(stderr) = tree.take_stderr() {
        forward(stderr, "stderr", sender);
    }
    let mut output = OutputWriter::create(files, spec.claude)?;
    let deadline = spec
        .timeout_ms
        .and_then(|millis| Instant::now().checked_add(Duration::from_millis(millis)));

    let ending = loop {
        match lines.recv_timeout(TICK) {
            Ok(piece) => output.write(&piece)?,
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
    Ok(ResultFile {
        claude: output.into_claude(),
        ..ending.into_result()
    })
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
            claude: None,
        }
    }
}

/// One piece of a line of output, as long as `MAX_LINE` at most.
struct Piece {
    stream: &'static str,
    text: String,
    continues: bool,
}

/// Sends the lines of `pipe` until it closes.
fn forward(pipe: impl Read + Send + 'static, stream: &'static str, sender: Sender<Piece>) {
    thread::spawn(move || {
        let mut lines = Lines::new(pipe);
        while let Ok(Some((text, continues))) = lines.next_line() {
            let piece = Piece {
                stream,
                text,
                continues,
            };
            if sender.send(piece).is_err() {
                break;
            }
        }
    });
}

/// Cuts a byte stream into pieces of at most `MAX_LINE` bytes without breaking characters.
struct Lines<R> {
    reader: BufReader<R>,
    // the start of a character cut off at the end of the previous piece
    carry: Vec<u8>,
}

impl<R: Read> Lines<R> {
    fn new(pipe: R) -> Self {
        Self {
            reader: BufReader::new(pipe),
            carry: Vec::new(),
        }
    }

    /// The next piece without its line ending and whether its line goes on, or `None` at the end.
    fn next_line(&mut self) -> io::Result<Option<(String, bool)>> {
        let mut buffer = std::mem::take(&mut self.carry);
        let read = (&mut self.reader)
            .take(MAX_LINE)
            .read_until(b'\n', &mut buffer)?;
        if read == 0 && buffer.is_empty() {
            return Ok(None);
        }
        let mut continues = false;
        if buffer.last() == Some(&b'\n') {
            buffer.pop();
        } else {
            match self.reader.fill_buf()?.first() {
                // the end of the stream
                None => {}
                // a line that fills a piece exactly ends here
                Some(b'\n') => self.reader.consume(1),
                Some(_) => {
                    continues = true;
                    if let Err(error) = std::str::from_utf8(&buffer)
                        && error.error_len().is_none()
                    {
                        self.carry = buffer.split_off(error.valid_up_to());
                    }
                }
            }
        }
        if !continues && buffer.last() == Some(&b'\r') {
            buffer.pop();
        }
        Ok(Some((
            String::from_utf8_lossy(&buffer).into_owned(),
            continues,
        )))
    }
}

/// Reads a Claude run's output while it is written: usage, errors and screenshots.
struct ClaudeWatch {
    stream: ClaudeStream,
    folder: PathBuf,
    screenshots: Vec<Screenshot>,
    // one per pipe, as the pieces of both interleave
    joiners: [LineJoiner; 2],
}

impl ClaudeWatch {
    fn new(files: &RunFiles) -> Self {
        Self {
            stream: ClaudeStream::new(),
            folder: files.artifacts(),
            screenshots: Vec::new(),
            joiners: [LineJoiner::default(), LineJoiner::default()],
        }
    }

    fn observe(&mut self, piece: &Piece) {
        let (index, pipe) = if piece.stream == "stdout" {
            (0, Pipe::Stdout)
        } else {
            (1, Pipe::Stderr)
        };
        match self.joiners[index].push(&piece.text, piece.continues) {
            Some(Joined::Line(line)) => self.line(pipe, &line),
            // never parsed in part, which could read as something else
            Some(Joined::Dropped) => self.stream.feed(pipe, DROPPED_LINE),
            None => {}
        }
    }

    fn line(&mut self, pipe: Pipe, text: &str) {
        self.stream.feed(pipe, text);
        if pipe == Pipe::Stdout {
            for image in images(text) {
                // a screenshot that cannot be kept must not fail the run
                let _ = self.keep(&image.media_type, &image.data, image.tool_use_id);
            }
        }
    }

    /// Lines cut off by the end of the output are read as they are.
    fn finish_lines(&mut self) {
        for (index, pipe) in [(0, Pipe::Stdout), (1, Pipe::Stderr)] {
            match self.joiners[index].finish() {
                Some(Joined::Line(line)) => self.line(pipe, &line),
                Some(Joined::Dropped) => self.stream.feed(pipe, DROPPED_LINE),
                None => {}
            }
        }
    }

    fn keep(
        &mut self,
        media_type: &str,
        data: &str,
        tool_use_id: Option<String>,
    ) -> io::Result<()> {
        if self.screenshots.len() >= MAX_SCREENSHOTS {
            return Ok(());
        }
        let bytes = STANDARD
            .decode(data.trim())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if bytes.len() > MAX_SCREENSHOT_BYTES {
            return Ok(());
        }
        let extension = match media_type {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            // the type is the model's word and is served back as it is
            _ => return Ok(()),
        };
        let file = format!("shot-{}.{extension}", self.screenshots.len() + 1);
        fs::create_dir_all(&self.folder)?;
        write_atomic(&self.folder.join(&file), &bytes)?;
        self.screenshots.push(Screenshot {
            tool_use_id,
            media_type: media_type.to_owned(),
            file,
            bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        });
        Ok(())
    }

    fn into_file(mut self) -> ClaudeFile {
        self.finish_lines();
        ClaudeFile::new(self.stream.finish(), &self.screenshots)
    }
}

/// Appends output lines and their offsets; a reader trusts a line once it has an index entry.
struct OutputWriter {
    output: File,
    index: File,
    offset: u64,
    seq: i64,
    claude: Option<ClaudeWatch>,
}

impl OutputWriter {
    fn create(files: &RunFiles, claude: bool) -> io::Result<Self> {
        Ok(Self {
            output: File::create(files.output())?,
            index: File::create(files.index())?,
            offset: 0,
            seq: 0,
            claude: claude.then(|| ClaudeWatch::new(files)),
        })
    }

    fn write(&mut self, piece: &Piece) -> io::Result<()> {
        self.seq += 1;
        let record = OutputRecord {
            seq: self.seq,
            stream: piece.stream.to_owned(),
            text: piece.text.clone(),
            continues: piece.continues,
        };
        let mut line = serde_json::to_vec(&record).map_err(io::Error::other)?;
        line.push(b'\n');
        self.output.write_all(&line)?;
        self.index.write_all(&self.offset.to_le_bytes())?;
        self.offset += u64::try_from(line.len()).map_err(io::Error::other)?;
        if let Some(claude) = &mut self.claude {
            claude.observe(piece);
        }
        Ok(())
    }

    fn drain(&mut self, lines: &Receiver<Piece>) -> io::Result<()> {
        while let Ok(piece) = lines.try_recv() {
            self.write(&piece)?;
        }
        Ok(())
    }

    fn finish(&mut self, lines: &Receiver<Piece>) -> io::Result<()> {
        let deadline = Instant::now() + DRAIN;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match lines.recv_timeout(left) {
                Ok(piece) => self.write(&piece)?,
                Err(_) => return Ok(()),
            }
        }
    }

    fn into_claude(self) -> Option<ClaudeFile> {
        self.claude.map(ClaudeWatch::into_file)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
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
            ..SpecFile::default()
        };
        fs::write(dir.join("spec.json"), serde_json::to_vec(&spec).unwrap()).unwrap();
        RunFiles::at(dir)
    }

    fn result(files: &RunFiles) -> ResultFile {
        read_json::<ResultFile>(&files.result()).unwrap().unwrap()
    }

    #[test]
    fn only_a_plain_claude_run_may_be_replaced_through_the_environment() {
        let claude = SpecFile {
            program: String::from("claude"),
            claude: true,
            ..SpecFile::default()
        };
        let replaced = |spec: &SpecFile, command: Option<&str>| {
            claude_override(spec, command.map(str::to_owned)).program
        };
        assert_eq!(replaced(&claude, Some("/bin/fake")), "/bin/fake");
        assert_eq!(replaced(&claude, Some(" ")), "claude");
        assert_eq!(replaced(&claude, None), "claude");
        let named = SpecFile {
            program: String::from("/opt/claude"),
            ..claude.clone()
        };
        assert_eq!(replaced(&named, Some("/bin/fake")), "/opt/claude");
        let plain = SpecFile {
            claude: false,
            ..claude
        };
        assert_eq!(replaced(&plain, Some("/bin/fake")), "claude");
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
            ..SpecFile::default()
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

    fn all_pieces(bytes: &[u8]) -> Vec<(String, bool)> {
        let mut lines = Lines::new(bytes);
        let mut all = Vec::new();
        while let Some(piece) = lines.next_line().unwrap() {
            all.push(piece);
        }
        all
    }

    fn all_lines(bytes: &[u8]) -> Vec<String> {
        all_pieces(bytes).into_iter().map(|piece| piece.0).collect()
    }

    #[test]
    fn marks_the_pieces_of_a_long_line_as_continuing() {
        let limit = usize::try_from(MAX_LINE).unwrap();
        let mut long = vec![b'x'; 2 * limit + 10];
        long.extend_from_slice(
            b"
short",
        );
        let marks: Vec<(usize, bool)> = all_pieces(&long)
            .iter()
            .map(|(text, continues)| (text.len(), *continues))
            .collect();
        assert_eq!(
            marks,
            vec![(limit, true), (limit, true), (10, false), (5, false)]
        );
        // a line of exactly one piece is no continuing line and leaves no empty line behind
        let mut exact = vec![b'y'; limit];
        exact.extend_from_slice(
            b"
next",
        );
        let marks: Vec<(usize, bool)> = all_pieces(&exact)
            .iter()
            .map(|(text, continues)| (text.len(), *continues))
            .collect();
        assert_eq!(marks, vec![(limit, false), (4, false)]);
        assert_eq!(
            all_pieces(&vec![b'z'; limit]),
            vec![("z".repeat(limit), false)]
        );
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
            ..SpecFile::default()
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
            ..SpecFile::default()
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

    #[cfg(windows)]
    #[test]
    fn a_relative_claude_program_is_found_in_the_run_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("fake.cmd"), "@echo from the batch\r\n").unwrap();
        let files = claude_spec(dir.path(), ".\\fake.cmd", &[]);
        supervise(files.dir()).unwrap();
        assert_eq!(result(&files).outcome, "succeeded");
        let lines = read_output(&files, 0, 10).unwrap();
        assert_eq!(lines[0].text.trim(), "from the batch");
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

    fn claude_spec(dir: &Path, program: &str, args: &[&str]) -> RunFiles {
        let spec = SpecFile {
            program: program.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: dir.to_path_buf(),
            claude: true,
            ..SpecFile::default()
        };
        fs::write(dir.join("spec.json"), serde_json::to_vec(&spec).unwrap()).unwrap();
        RunFiles::at(dir)
    }

    /// A Claude run output kept in a file the shell prints, as JSON quoting in cmd is a trap.
    fn replay(dir: &Path, lines: &[&str]) -> RunFiles {
        fs::write(
            dir.join("replay.jsonl"),
            lines.join(
                "
",
            ) + "
",
        )
        .unwrap();
        let (program, mut args) = shell(if cfg!(windows) {
            "type replay.jsonl"
        } else {
            "cat replay.jsonl"
        });
        let spec = SpecFile {
            program,
            args: std::mem::take(&mut args),
            cwd: dir.to_path_buf(),
            claude: true,
            ..SpecFile::default()
        };
        fs::write(dir.join("spec.json"), serde_json::to_vec(&spec).unwrap()).unwrap();
        RunFiles::at(dir)
    }

    #[test]
    fn a_claude_run_leaves_its_usage_denials_and_screenshots_in_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
        let image = format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","content":[{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{png}"}}}}]}}]}}}}"#
        );
        let files = replay(
            dir.path(),
            &[
                r#"{"type":"system","subtype":"init","session_id":"s-1"}"#,
                &image,
                r#"{"type":"result","subtype":"success","is_error":false,"num_turns":3,"total_cost_usd":0.5,"usage":{"input_tokens":10,"output_tokens":20},"permission_denials":[{"tool_name":"Bash"}]}"#,
            ],
        );
        supervise(files.dir()).unwrap();
        let report = result(&files);
        assert_eq!(report.outcome, "succeeded");
        let claude = report.claude.unwrap();
        assert_eq!(claude.session_id.as_deref(), Some("s-1"));
        assert!(claude.has_result);
        let usage = claude.usage.unwrap();
        assert_eq!(
            (
                usage.cost_usd,
                usage.input_tokens,
                usage.output_tokens,
                usage.num_turns
            ),
            (Some(0.5), Some(10), Some(20), Some(3))
        );
        assert_eq!(claude.denied, vec![(String::from("Bash"), 1)]);
        assert_eq!(claude.screenshots.len(), 1);
        let shot = &claude.screenshots[0];
        assert_eq!(
            (shot.media_type.as_str(), shot.file.as_str()),
            ("image/png", "shot-1.png")
        );
        assert_eq!(shot.tool_use_id.as_deref(), Some("t1"));
        let kept = fs::read(files.artifacts().join("shot-1.png")).unwrap();
        assert_eq!(kept.len() as u64, shot.bytes);
        assert_eq!(&kept[1..4], b"PNG");
    }

    fn image_line(media_type: &str, data: &str) -> String {
        format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","content":[{{"type":"image","source":{{"type":"base64","media_type":"{media_type}","data":"{data}"}}}}]}}]}}}}"#
        )
    }

    #[test]
    fn a_screenshot_line_over_the_piece_size_is_still_saved() {
        let dir = tempfile::tempdir().unwrap();
        // about 150 KB of bytes, which is over 200 KB of base64
        let data = "A".repeat(200_000);
        let image = image_line("image/png", &data);
        assert!(image.len() > 2 * usize::try_from(MAX_LINE).unwrap());
        let files = replay(dir.path(), &[&image]);
        supervise(files.dir()).unwrap();
        let claude = result(&files).claude.unwrap();
        assert_eq!(claude.screenshots.len(), 1);
        assert_eq!(claude.screenshots[0].bytes, 150_000);
        assert_eq!(
            fs::read(files.artifacts().join("shot-1.png"))
                .unwrap()
                .len(),
            150_000
        );
        assert!(!claude.error_text.contains("AAAA"));
        let lines = read_output(&files, 0, 10).unwrap();
        assert!(lines.len() > 1 && lines[0].continues && !lines[lines.len() - 1].continues);
    }

    #[test]
    fn a_result_line_over_the_piece_size_still_yields_usage_and_denials() {
        let dir = tempfile::tempdir().unwrap();
        let padding = "x".repeat(150_000);
        let line = format!(
            r#"{{"type":"result","subtype":"success","is_error":false,"num_turns":4,"total_cost_usd":1.5,"usage":{{"input_tokens":11,"output_tokens":22}},"result":"{padding}","permission_denials":[{{"tool_name":"Bash"}}]}}"#
        );
        let files = replay(dir.path(), &[&line]);
        supervise(files.dir()).unwrap();
        let claude = result(&files).claude.unwrap();
        assert!(claude.has_result);
        let usage = claude.usage.unwrap();
        assert_eq!(
            (usage.cost_usd, usage.input_tokens, usage.num_turns),
            (Some(1.5), Some(11), Some(4))
        );
        assert_eq!(claude.denied, vec![(String::from("Bash"), 1)]);
        assert!(!claude.error_text.contains("xxxx"));
    }

    #[test]
    fn only_known_image_types_are_kept_as_screenshots() {
        let dir = tempfile::tempdir().unwrap();
        let lines = [
            image_line("text/html", "AAAA"),
            image_line("image/svg+xml", "AAAA"),
            image_line("image/webp", "AAAA"),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let files = replay(dir.path(), &refs);
        supervise(files.dir()).unwrap();
        let shots = result(&files).claude.unwrap().screenshots;
        assert_eq!(shots.len(), 1);
        assert_eq!(
            (shots[0].media_type.as_str(), shots[0].file.as_str()),
            ("image/webp", "shot-1.webp")
        );
    }

    #[test]
    fn plain_runs_leave_no_claude_block() {
        let dir = tempfile::tempdir().unwrap();
        let files = prepare(dir.path(), "echo hi", None);
        supervise(files.dir()).unwrap();
        assert_eq!(result(&files).claude, None);
    }

    #[test]
    fn a_claude_run_that_cannot_start_still_reports_an_empty_claude_block() {
        let dir = tempfile::tempdir().unwrap();
        let files = claude_spec(dir.path(), "nullradix-no-such-program", &[]);
        supervise(files.dir()).unwrap();
        let report = result(&files);
        assert_eq!(report.exit_code, Some(127));
        let claude = report.claude.unwrap();
        assert!(!claude.has_result);
        assert_eq!(claude.usage, None);
    }

    #[test]
    fn the_prompt_reaches_the_program_over_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let spec = SpecFile {
            program: String::from(if cfg!(windows) { "findstr" } else { "cat" }),
            args: if cfg!(windows) {
                vec![String::from("/R"), String::from("^")]
            } else {
                Vec::new()
            },
            cwd: dir.path().to_path_buf(),
            stdin: Some(String::from(
                "b
a
",
            )),
            ..SpecFile::default()
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
        let texts: Vec<&str> = lines.iter().map(|line| line.text.trim()).collect();
        assert_eq!(texts, vec!["b", "a"]);
    }

    #[test]
    fn the_environment_changes_apply_in_order_removals_defaults_then_sets() {
        let env = EnvFile {
            set: BTreeMap::from([(String::from("A"), String::from("set"))]),
            remove: vec![String::from("GONE")],
            defaults: BTreeMap::from([
                (String::from("A"), String::from("default")),
                (String::from("B"), String::from("default")),
                (String::from("HAS"), String::from("default")),
            ]),
        };
        let mut command = Command::new("x");
        apply_env(&mut command, &env, &|name| {
            (name == "HAS").then(|| OsString::from("own"))
        });
        let applied: BTreeMap<String, Option<String>> = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            applied,
            BTreeMap::from([
                (String::from("A"), Some(String::from("set"))),
                (String::from("B"), Some(String::from("default"))),
                (String::from("GONE"), None),
            ])
        );
    }
}
