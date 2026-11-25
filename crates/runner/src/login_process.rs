//! Runs `claude auth login` as a child process of the daemon.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::login::{LoginProcess, LoginRunner, LoginSpec, claude_not_found};
use nullradix_os::ProcessTree;
use tokio::sync::{mpsc, oneshot};

use crate::accounts::expand_home;

const POLL: Duration = Duration::from_millis(100);
/// Variables that would make Claude Code log in somewhere else than in the account's folder.
const REMOVED: [&str; 3] = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDECODE"];

#[derive(Debug, Clone)]
pub struct LocalLoginRunner {
    program: String,
    prefix: Vec<String>,
    /// What `~` stands for in an account folder.
    home: Option<PathBuf>,
    /// Folder for the default account, which otherwise follows the environment of the daemon.
    default_folder: Option<PathBuf>,
}

/// How to start `command`; on Windows `claude` is a batch file that only `cmd` finds.
fn launcher(windows: bool, command: &str) -> (String, Vec<String>) {
    let executable = command.to_ascii_lowercase().ends_with(".exe");
    if windows && !executable {
        (
            String::from("cmd"),
            vec![String::from("/C"), command.to_owned()],
        )
    } else {
        (command.to_owned(), Vec::new())
    }
}

impl LocalLoginRunner {
    /// The `claude` of the path, or the command named by `NULLRADIX_CLAUDE`.
    #[must_use]
    pub fn claude() -> Self {
        let command = std::env::var("NULLRADIX_CLAUDE")
            .ok()
            .filter(|command| !command.trim().is_empty())
            .unwrap_or_else(|| String::from("claude"));
        Self::command(&command)
    }

    #[must_use]
    pub fn command(command: &str) -> Self {
        let (program, prefix) = launcher(cfg!(windows), command);
        Self::with_launcher(program, prefix)
    }

    /// Starts `program` with `prefix` before the login arguments, so tests can stand in for Claude.
    #[must_use]
    pub fn with_launcher(program: String, prefix: Vec<String>) -> Self {
        Self {
            program,
            prefix,
            home: nullradix_os::home_dir(),
            default_folder: None,
        }
    }

    /// Uses `home` for `~` in account folders, as the folders and usage of the daemon do.
    #[must_use]
    pub fn with_home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }

    /// Logs the default account in in this folder, which then exists before Claude starts.
    #[must_use]
    pub fn with_default_folder(mut self, folder: Option<PathBuf>) -> Self {
        self.default_folder = folder;
        self
    }

    /// The folder Claude Code logs in to; none when it follows the environment of the daemon.
    fn folder_of(&self, spec: &LoginSpec) -> Option<PathBuf> {
        match &spec.config_dir {
            Some(dir) => Some(expand_home(self.home.as_deref(), dir.as_str())),
            None => self.default_folder.clone(),
        }
    }

    fn build(&self, spec: &LoginSpec) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.prefix)
            .args(["auth", "login", "--claudeai"]);
        if let Some(email) = &spec.email {
            command.args(["--email", email]);
        }
        for name in REMOVED {
            command.env_remove(name);
        }
        if let Some(folder) = self.folder_of(spec) {
            command.env("CLAUDE_CONFIG_DIR", folder);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

fn lines(pipe: impl std::io::Read + Send + 'static, output: mpsc::Sender<String>) {
    thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            if output.blocking_send(format!("{line}\n")).is_err() {
                return;
            }
        }
    });
}

#[async_trait]
impl LoginRunner for LocalLoginRunner {
    async fn start(&self, spec: &LoginSpec) -> Result<LoginProcess, AppError> {
        if spec.config_dir.is_none()
            && let Some(folder) = self.default_folder.clone()
        {
            tokio::task::spawn_blocking(move || std::fs::create_dir_all(folder))
                .await
                .map_err(AppError::internal)?
                .map_err(AppError::internal)?;
        }
        let mut command = self.build(spec);
        let mut tree = ProcessTree::spawn(&mut command).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                claude_not_found(&self.program)
            } else {
                AppError::internal(error)
            }
        })?;
        let (output, lines_in) = mpsc::channel::<String>(64);
        let (input, mut typed) = mpsc::channel::<String>(8);
        let (exit, exited) = oneshot::channel();
        if let Some(stdout) = tree.take_stdout() {
            lines(stdout, output.clone());
        }
        if let Some(stderr) = tree.take_stderr() {
            lines(stderr, output.clone());
        }
        drop(output);
        if let Some(mut stdin) = tree.take_stdin() {
            thread::spawn(move || {
                while let Some(text) = typed.blocking_recv() {
                    if stdin
                        .write_all(text.as_bytes())
                        .and_then(|()| stdin.flush())
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
        let tree = Arc::new(Mutex::new(tree));
        let watched = Arc::clone(&tree);
        thread::spawn(move || {
            let code = loop {
                let status = watched
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .try_wait();
                match status {
                    Ok(Some(status)) => break status.code(),
                    Ok(None) => thread::sleep(POLL),
                    Err(_) => break None,
                }
            };
            let _ = exit.send(code);
        });
        Ok(LoginProcess {
            output: lines_in,
            exit: exited,
            input,
            kill: Box::new(move || {
                let _ = tree.lock().unwrap_or_else(PoisonError::into_inner).kill();
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// A stand-in for Claude Code: a shell running `script`, which the login arguments follow.
    fn shell(script: &str) -> LocalLoginRunner {
        if cfg!(windows) {
            LocalLoginRunner::with_launcher(
                String::from("cmd"),
                vec![String::from("/C"), script.to_owned()],
            )
        } else {
            LocalLoginRunner::with_launcher(
                String::from("sh"),
                vec![
                    String::from("-c"),
                    script.to_owned(),
                    String::from("claude"),
                ],
            )
        }
    }

    fn spec() -> LoginSpec {
        LoginSpec {
            config_dir: None,
            email: None,
        }
    }

    async fn collect(output: &mut mpsc::Receiver<String>) -> String {
        let mut text = String::new();
        while let Some(line) = output.recv().await {
            text.push_str(&line);
        }
        text
    }

    #[tokio::test]
    async fn collects_output_and_reports_a_clean_exit() {
        let mut process = shell("echo https://claude.ai/login")
            .start(&spec())
            .await
            .unwrap();
        let text = collect(&mut process.output).await;
        assert!(text.starts_with("https://claude.ai/login"), "{text}");
        assert_eq!((&mut process.exit).await.unwrap(), Some(0));
    }

    #[tokio::test]
    async fn the_arguments_of_a_login_follow_the_launcher() {
        let runner = shell("echo args");
        let with_email = LoginSpec {
            config_dir: None,
            email: Some(String::from("me@example.com")),
        };
        let mut process = runner.start(&with_email).await.unwrap();
        let text = collect(&mut process.output).await;
        assert!(
            text.contains("auth login --claudeai --email me@example.com"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_failing_login_reports_its_exit_code_and_stderr() {
        let script = if cfg!(windows) {
            "echo broken 1>&2& exit 3"
        } else {
            "echo broken 1>&2; exit 3"
        };
        let mut process = shell(script).start(&spec()).await.unwrap();
        let text = collect(&mut process.output).await;
        assert!(text.contains("broken"), "{text}");
        assert_eq!((&mut process.exit).await.unwrap(), Some(3));
    }

    #[tokio::test]
    async fn typed_text_reaches_the_process_and_closing_the_input_ends_it() {
        let echo = if cfg!(windows) {
            "findstr /V ZZZZ & rem"
        } else {
            "cat"
        };
        let mut process = shell(echo).start(&spec()).await.unwrap();
        process.input.send("the-code\n".to_owned()).await.unwrap();
        drop(process.input);
        let text = collect(&mut process.output).await;
        assert!(text.starts_with("the-code"), "{text}");
        assert_eq!((&mut process.exit).await.unwrap(), Some(0));
    }

    #[tokio::test]
    async fn killing_ends_a_login_that_waits() {
        let wait = if cfg!(windows) {
            "ping -n 60 127.0.0.1"
        } else {
            "sleep 60"
        };
        let process = shell(wait).start(&spec()).await.unwrap();
        let started = Instant::now();
        (process.kill)();
        let result = tokio::time::timeout(Duration::from_secs(20), process.exit)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(result, Some(0));
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[tokio::test]
    async fn a_missing_program_is_claude_not_found() {
        let runner =
            LocalLoginRunner::with_launcher(String::from("nullradix-no-such-claude"), Vec::new());
        let error = runner.start(&spec()).await.err().unwrap();
        assert_eq!(error.code(), "claude-not-found");
    }

    fn config_dir_of(command: &Command) -> Option<std::ffi::OsString> {
        command
            .get_envs()
            .find(|(name, _)| *name == "CLAUDE_CONFIG_DIR")
            .and_then(|(_, value)| value.map(std::ffi::OsStr::to_os_string))
    }

    #[test]
    fn an_isolated_home_serves_tilde_folders_and_the_default_account_alike() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("data");
        let runner = LocalLoginRunner::with_launcher(String::from("claude"), Vec::new())
            .with_home(Some(home.clone()))
            .with_default_folder(Some(home.join(".claude")));
        let tilde = LoginSpec {
            config_dir: Some(nullradix_domain::ConfigDir::parse("~/.mcacc/work").unwrap()),
            email: None,
        };
        assert_eq!(
            config_dir_of(&runner.build(&tilde)),
            Some(home.join(".mcacc/work").into_os_string())
        );
        assert_eq!(
            config_dir_of(&runner.build(&spec())),
            Some(home.join(".claude").into_os_string())
        );
    }

    #[test]
    fn the_default_account_follows_the_daemon_without_an_isolated_home() {
        let runner = LocalLoginRunner::with_launcher(String::from("claude"), Vec::new());
        assert_eq!(config_dir_of(&runner.build(&spec())), None);
    }

    #[tokio::test]
    async fn the_default_folder_exists_when_the_login_starts() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("data/.claude");
        let runner = shell("echo hi").with_default_folder(Some(folder.clone()));
        let mut process = runner.start(&spec()).await.unwrap();
        assert!(folder.is_dir());
        collect(&mut process.output).await;
    }
    #[test]
    fn windows_finds_the_batch_file_of_claude_through_cmd() {
        assert_eq!(
            launcher(true, "claude"),
            (
                String::from("cmd"),
                vec![String::from("/C"), String::from("claude")]
            )
        );
        assert_eq!(
            launcher(true, "C:\\tools\\claude.EXE"),
            (String::from("C:\\tools\\claude.EXE"), Vec::new())
        );
        assert_eq!(
            launcher(false, "claude"),
            (String::from("claude"), Vec::new())
        );
    }

    #[test]
    fn accounts_log_in_in_their_own_folder_and_without_competing_credentials() {
        let runner = LocalLoginRunner::with_launcher(String::from("claude"), Vec::new());
        let own = LoginSpec {
            config_dir: Some(nullradix_domain::ConfigDir::parse("/data/accounts/max").unwrap()),
            email: None,
        };
        let command = runner.build(&own);
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains(&(
            String::from("CLAUDE_CONFIG_DIR"),
            Some(String::from("/data/accounts/max"))
        )));
        for name in REMOVED {
            assert!(envs.contains(&(name.to_owned(), None)), "{name}");
        }
        let default = runner.build(&spec());
        assert!(
            default
                .get_envs()
                .all(|(name, _)| name != "CLAUDE_CONFIG_DIR")
        );
    }
}
