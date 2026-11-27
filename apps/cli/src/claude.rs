use std::env;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nullradix_os::ProcessTree;

const TIMEOUT: Duration = Duration::from_secs(120);
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
    let path = path?;
    for dir in env::split_paths(path).filter(|dir| dir.is_absolute()) {
        for extension in extensions {
            let candidate = dir.join(format!("claude{extension}"));
            if candidate.is_file() {
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
fn drain(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        if let Some(mut pipe) = pipe {
            let mut chunk = [0_u8; 8192];
            while let Ok(read) = pipe.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                let room = OUTPUT_LIMIT.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..read.min(room)]);
            }
        }
        String::from_utf8_lossy(&kept).into_owned()
    })
}

/// Runs `program` and, past `timeout`, kills it with everything it started.
fn run_program(
    program: &Path,
    timeout: Duration,
    config_dir: Option<&Path>,
    args: &[&str],
) -> Answer {
    let mut command = Command::new(program);
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
    let text = format!(
        "{}{}",
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default()
    );
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
        let here = tempfile::tempdir_in(".").unwrap();
        fs::write(here.path().join("claude.cmd"), "").unwrap();
        let relative = here.path().file_name().unwrap().to_owned();
        let path =
            env::join_paths([PathBuf::new(), PathBuf::from("."), PathBuf::from(relative)]).unwrap();

        assert_eq!(resolve(Some(&path), &extensions(&[".cmd", ""])), None);
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

    fn slow_script(dir: &Path) -> PathBuf {
        if cfg!(windows) {
            let script = dir.join("claude.cmd");
            fs::write(&script, "@echo off\r\nping -n 9 127.0.0.1\r\n").unwrap();
            script
        } else {
            let script = dir.join("claude");
            fs::write(&script, "#!/bin/sh\nsleep 8\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            }
            script
        }
    }

    #[test]
    fn a_timeout_ends_the_script_and_the_program_it_started() {
        let temp = tempfile::tempdir().unwrap();
        let script = slow_script(temp.path());
        let started = Instant::now();

        let answer = run_program(&script, Duration::from_millis(300), None, &[]);

        assert!(!answer.ok);
        assert!(answer.text.contains("did not finish"), "{}", answer.text);
        // a surviving child keeps the output pipes open and would hold the answer for seconds
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn output_beyond_the_cap_is_discarded_without_blocking_the_writer() {
        let big = std::io::repeat(b'x').take((OUTPUT_LIMIT * 3) as u64);

        let kept = drain(Some(big)).join().unwrap();

        assert_eq!(kept.len(), OUTPUT_LIMIT);
    }
}
