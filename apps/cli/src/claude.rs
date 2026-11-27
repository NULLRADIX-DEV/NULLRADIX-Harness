use std::env;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(20);
const OUTPUT_LIMIT: u64 = 1024 * 1024;

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

/// The program that `claude` names on `path`, trying each extension in turn.
pub(crate) fn resolve(path: Option<&OsStr>, extensions: &[String]) -> Option<PathBuf> {
    let path = path?;
    for dir in env::split_paths(path) {
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
        .filter(|extension| !extension.is_empty())
        .map(str::to_ascii_lowercase)
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

fn drain(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(pipe) = pipe {
            let _ = pipe.take(OUTPUT_LIMIT).read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

impl ClaudeCli for SystemClaude {
    fn run(&self, config_dir: Option<&Path>, args: &[&str]) -> Answer {
        let Some(program) = &self.program else {
            return Answer::failed("the command `claude` was not found on PATH");
        };
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = config_dir {
            command.env("CLAUDE_CONFIG_DIR", dir);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return Answer::failed(error.to_string()),
        };
        let out = drain(child.stdout.take());
        let err = drain(child.stderr.take());
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() > self.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Answer::failed(format!(
                        "claude did not finish within {} seconds",
                        self.timeout.as_secs_f32()
                    ));
                }
                Ok(None) => thread::sleep(POLL),
                Err(error) => return Answer::failed(error.to_string()),
            }
        };
        let text = format!(
            "{}{}",
            out.join().unwrap_or_default(),
            err.join().unwrap_or_default()
        );
        Answer {
            ok: status.success(),
            text,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

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
        let extensions = [String::from(".exe"), String::from(".cmd")];

        let found = resolve(Some(&path), &extensions).unwrap();

        assert_eq!(found, first.join("claude.cmd"));
        assert_eq!(resolve(Some(&path), &[String::from(".bat")]), None);
        assert_eq!(resolve(None, &extensions), None);
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

    #[test]
    fn a_program_that_never_ends_is_killed_at_the_deadline() {
        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            ("ping", &["-n", "20", "127.0.0.1"])
        } else {
            ("sleep", &["20"])
        };
        let claude = SystemClaude {
            program: Some(PathBuf::from(program)),
            timeout: Duration::from_millis(200),
        };
        let started = Instant::now();

        let answer = claude.run(None, args);

        assert!(!answer.ok);
        assert!(answer.text.contains("did not finish"), "{}", answer.text);
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
