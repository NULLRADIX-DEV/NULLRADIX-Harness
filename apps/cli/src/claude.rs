use std::env;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use nullradix_os::ProcessTree;

const TIMEOUT: Duration = Duration::from_secs(120);
const GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(20);
const OUTPUT_LIMIT: usize = 1024 * 1024;
/// Only these can be started directly; anything else a shell would have to interpret.
const RUNNABLE: [&str; 4] = [".exe", ".com", ".cmd", ".bat"];

/// What `claude` answered to one call.
pub(crate) struct Answer {
    pub(crate) ok: bool,
    pub(crate) text: String,
}

impl Answer {
    fn failed(text: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: text.into(),
        }
    }
}

/// The command line of Claude Code; a fake stands in for it in tests.
pub(crate) trait ClaudeCli {
    /// Runs `claude <args>` with `CLAUDE_CONFIG_DIR` set to `config_dir` when given.
    fn run(&self, config_dir: Option<&Path>, args: &[&str]) -> Answer;
}

/// The program `claude` names in the absolute folders of `path`.
pub(crate) fn resolve(path: Option<&OsStr>, extensions: &[String]) -> Option<PathBuf> {
    resolve_in(None, path, extensions)
}

/// Like `resolve`; a relative entry would count against `base`, but none ever does.
fn resolve_in(base: Option<&Path>, path: Option<&OsStr>, extensions: &[String]) -> Option<PathBuf> {
    let path = path?;
    for dir in env::split_paths(path).filter(|dir| dir.is_absolute()) {
        for extension in extensions {
            let candidate = dir.join(format!("claude{extension}"));
            let checked = base.map_or_else(|| candidate.clone(), |base| base.join(&candidate));
            if checked.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn path_extensions() -> Vec<String> {
    if !cfg!(windows) {
        return vec![String::new()];
    }
    let listed = env::var("PATHEXT").unwrap_or_else(|_| String::from(".COM;.EXE;.BAT;.CMD"));
    listed
        .split(';')
        .map(str::to_ascii_lowercase)
        .filter(|extension| RUNNABLE.contains(&extension.as_str()))
        .collect()
}

/// The installed Claude Code, started directly so no shell reads its arguments.
pub(crate) struct SystemClaude {
    program: Option<PathBuf>,
    timeout: Duration,
}

impl SystemClaude {
    pub(crate) fn find() -> Self {
        Self {
            program: resolve(env::var_os("PATH").as_deref(), &path_extensions()),
            timeout: TIMEOUT,
        }
    }
}

/// Keeps the first part of what a pipe delivers and discards the rest, so the writer never blocks.
fn drain(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut cut = false;
        if let Some(mut pipe) = pipe {
            let mut chunk = [0_u8; 8192];
            while let Ok(read) = pipe.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                let room = OUTPUT_LIMIT.saturating_sub(kept.len());
                cut |= read > room;
                kept.extend_from_slice(&chunk[..read.min(room)]);
            }
        }
        let mut text = String::from_utf8_lossy(&kept).into_owned();
        if cut {
            text.push_str("\n[output truncated]");
        }
        let _ = sender.send(text);
    });
    receiver
}

/// What a reader kept; a descendant that outlived its tree and holds the pipe cannot hold us.
fn collect(reader: &mpsc::Receiver<String>, grace: Duration) -> String {
    reader
        .recv_timeout(grace)
        .unwrap_or_else(|_| "[output not collected]".to_owned())
}

/// Runs `program` and, past `timeout`, kills it with everything it started.
fn run_program(
    program: &Path,
    timeout: Duration,
    config_dir: Option<&Path>,
    args: &[&str],
) -> Answer {
    let mut command = ProcessTree::command(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = config_dir {
        command.env("CLAUDE_CONFIG_DIR", dir);
    }
    let mut tree = match ProcessTree::spawn(&mut command) {
        Ok(tree) => tree,
        Err(error) => return Answer::failed(error.to_string()),
    };
    let out = drain(tree.take_stdout());
    let err = drain(tree.take_stderr());
    let started = Instant::now();
    let finished = loop {
        match tree.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() > timeout => break Err(None),
            Ok(None) => thread::sleep(POLL),
            Err(error) => break Err(Some(error.to_string())),
        }
    };
    if finished.is_err() {
        let _ = tree.kill();
        let _ = tree.wait();
    }
    let text = format!("{}{}", collect(&out, GRACE), collect(&err, GRACE));
    match finished {
        Ok(status) => Answer {
            ok: status.success(),
            text,
        },
        Err(Some(error)) => Answer::failed(error),
        Err(None) => Answer::failed(format!(
            "claude did not finish within {} seconds",
            timeout.as_secs_f32()
        )),
    }
}

impl ClaudeCli for SystemClaude {
    fn run(&self, config_dir: Option<&Path>, args: &[&str]) -> Answer {
        match &self.program {
            Some(program) => run_program(program, self.timeout, config_dir, args),
            None => Answer::failed("the command `claude` was not found on PATH"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn extensions(list: &[&str]) -> Vec<String> {
        list.iter()
            .map(|extension| (*extension).to_owned())
            .collect()
    }

    #[test]
    fn finds_the_first_extension_in_the_first_folder_that_has_one() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("claude.cmd"), "").unwrap();
        fs::write(second.join("claude.exe"), "").unwrap();
        let path = env::join_paths([&first, &second]).unwrap();
        let list = extensions(&[".exe", ".cmd"]);

        let found = resolve(Some(&path), &list).unwrap();

        assert_eq!(found, first.join("claude.cmd"));
        assert_eq!(resolve(Some(&path), &extensions(&[".bat"])), None);
        assert_eq!(resolve(None, &list), None);
    }

    #[test]
    fn empty_and_relative_entries_never_find_a_program() {
        let cwd = tempfile::tempdir().unwrap();
        fs::write(cwd.path().join("claude.cmd"), "").unwrap();
        fs::create_dir_all(cwd.path().join("sub")).unwrap();
        fs::write(cwd.path().join("sub").join("claude.cmd"), "").unwrap();
        let path =
            env::join_paths([PathBuf::new(), PathBuf::from("."), PathBuf::from("sub")]).unwrap();
        let list = extensions(&[".cmd", ""]);

        assert_eq!(resolve_in(Some(cwd.path()), Some(&path), &list), None);
        let absolute = env::join_paths([cwd.path()]).unwrap();
        assert!(resolve_in(Some(cwd.path()), Some(&absolute), &list).is_some());
    }

    #[test]
    fn only_programs_that_can_start_directly_are_taken_from_pathext() {
        if cfg!(windows) {
            let list = path_extensions();
            assert!(
                list.iter().all(|e| RUNNABLE.contains(&e.as_str())),
                "{list:?}"
            );
        }
        assert!(!RUNNABLE.contains(&".vbs"));
    }

    #[test]
    fn a_missing_program_is_an_answer_not_a_panic() {
        let claude = SystemClaude {
            program: None,
            timeout: TIMEOUT,
        };

        let answer = claude.run(None, &["--version"]);

        assert!(!answer.ok);
        assert!(answer.text.contains("not found"));
    }

    /// A script whose grandchild notes its start and writes `late.txt` after 8 s.
    fn script_with_grandchild(dir: &Path) -> PathBuf {
        if cfg!(windows) {
            let script = dir.join("claude.cmd");
            let body = "@echo off\r\nstart /b cmd /c \"echo started> \"%~dp0started.txt\" & ping -n 9 127.0.0.1 >nul & echo late> \"%~dp0late.txt\"\"\r\nping -n 30 127.0.0.1\r\n";
            fs::write(&script, body).unwrap();
            script
        } else {
            let script = dir.join("claude");
            let body = "#!/bin/sh\nd=$(dirname \"$0\")\n(echo started > \"$d/started.txt\"; sleep 8; echo late > \"$d/late.txt\") &\nsleep 30\n";
            fs::write(&script, body).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            }
            script
        }
    }

    fn run_once_started(script: &Path) -> Answer {
        let answer = run_program(script, Duration::from_secs(6), None, &[]);
        // a script another thread was still writing cannot start; try once more
        if answer.text.contains("busy") {
            return run_program(script, Duration::from_secs(6), None, &[]);
        }
        answer
    }

    #[test]
    fn a_timeout_ends_the_script_and_the_program_it_started() {
        let temp = tempfile::tempdir().unwrap();
        let script = script_with_grandchild(temp.path());
        let marker = temp.path().join("started.txt");
        let runner = thread::spawn(move || run_once_started(&script));

        let deadline = Instant::now() + Duration::from_secs(20);
        while !marker.is_file() {
            assert!(Instant::now() < deadline, "the script never started");
            thread::sleep(POLL);
        }
        let answer = runner.join().unwrap();
        assert!(!answer.ok);
        assert!(answer.text.contains("did not finish"), "{}", answer.text);

        // the grandchild writes 8 s after its start; a survivor would have written by now
        thread::sleep(Duration::from_secs(9));
        assert!(!temp.path().join("late.txt").exists());
    }

    #[test]
    fn output_beyond_the_cap_is_discarded_without_blocking_the_writer() {
        let big = std::io::repeat(b'x').take((OUTPUT_LIMIT * 3) as u64);

        let kept = collect(&drain(Some(big)), Duration::from_secs(10));

        let text = kept.strip_suffix("\n[output truncated]").unwrap();
        assert_eq!(text.len(), OUTPUT_LIMIT);
    }

    struct Stuck;

    impl Read for Stuck {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            thread::sleep(Duration::from_secs(30));
            Ok(0)
        }
    }

    #[test]
    fn a_reader_held_open_by_a_stray_descendant_cannot_hold_the_answer() {
        let started = Instant::now();

        let kept = collect(&drain(Some(Stuck)), Duration::from_millis(100));

        assert_eq!(kept, "[output not collected]");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
