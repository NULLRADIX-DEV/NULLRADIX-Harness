#![cfg_attr(
    any(windows, target_os = "macos"),
    allow(dead_code, reason = "only Linux and other Unix systems use it")
)]

use std::fmt;
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::{SecretBackend, SecretError};

const NAME: &str = "Secret Service";
const PROGRAM: &str = "secret-tool";
const LIMIT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(20);
const PROBE_KEY: &str = "nullradix-probe";

/// What a finished `secret-tool` call said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Reply {
    pub(super) success: bool,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

type Runner = Box<dyn Fn(&[String], Option<&[u8]>) -> io::Result<Reply> + Send + Sync>;

/// Talks to the Secret Service through `secret-tool`, secrets only ever on its stdin.
pub(super) struct SecretTool {
    service: String,
    run: Runner,
}

impl fmt::Debug for SecretTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretTool")
            .field("service", &self.service)
            .finish_non_exhaustive()
    }
}

fn run_program(args: &[String], input: Option<&[u8]>) -> io::Result<Reply> {
    let mut child = Command::new(PROGRAM)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take()
        && let Some(bytes) = input
    {
        // a program that exits early closes the pipe; its status tells why
        let _ = stdin.write_all(bytes);
    }
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(child.stdout.take().map(|pipe| Box::new(pipe) as _));
    let stderr = drain(child.stderr.take().map(|pipe| Box::new(pipe) as _));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "secret-tool did not answer in time",
            ));
        }
        thread::sleep(POLL);
    };
    let text = |reader: thread::JoinHandle<Vec<u8>>| {
        String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned()
    };
    Ok(Reply {
        success: status.success(),
        stdout: text(stdout),
        stderr: text(stderr),
    })
}

impl SecretTool {
    /// The Secret Service through the installed `secret-tool`, when a session answers.
    pub(super) fn detect(service: &str) -> Option<Self> {
        let tool = Self::with_runner(service, Box::new(run_program));
        tool.get(PROBE_KEY).ok().map(|_| tool)
    }

    pub(super) fn with_runner(service: &str, run: Runner) -> Self {
        Self {
            service: service.to_owned(),
            run,
        }
    }

    fn attributes(&self, key: &str) -> Vec<String> {
        ["service", &self.service, "account", key]
            .map(str::to_owned)
            .to_vec()
    }

    fn call(
        &self,
        command: &str,
        extra: &[&str],
        key: &str,
        input: Option<&[u8]>,
    ) -> Result<Reply, SecretError> {
        let mut args = vec![command.to_owned()];
        args.extend(extra.iter().map(|arg| (*arg).to_owned()));
        args.extend(self.attributes(key));
        (self.run)(&args, input).map_err(|source| SecretError::Backend {
            backend: NAME,
            source,
        })
    }

    fn rejected(reply: &Reply) -> SecretError {
        SecretError::Backend {
            backend: NAME,
            source: io::Error::other(reply.stderr.trim().to_owned()),
        }
    }
}

impl SecretBackend for SecretTool {
    fn name(&self) -> &'static str {
        NAME
    }

    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        let reply = self.call("lookup", &[], key, None)?;
        if reply.success {
            Ok(Some(reply.stdout))
        } else if reply.stderr.trim().is_empty() {
            // an unknown item ends with status 1 and no message
            Ok(None)
        } else {
            Err(Self::rejected(&reply))
        }
    }

    fn put(&self, key: &str, value: &str) -> Result<(), SecretError> {
        let label = format!("--label={} {key}", self.service);
        let reply = self.call("store", &[&label], key, Some(value.as_bytes()))?;
        if reply.success {
            Ok(())
        } else {
            Err(Self::rejected(&reply))
        }
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        let reply = self.call("clear", &[], key, None)?;
        if reply.success || reply.stderr.trim().is_empty() {
            Ok(())
        } else {
            Err(Self::rejected(&reply))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;

    type Calls = Arc<Mutex<Vec<(Vec<String>, Option<Vec<u8>>)>>>;

    /// Behaves like `secret-tool` on an unlocked keyring and records every call.
    fn fake() -> (SecretTool, Calls) {
        let items: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        let calls: Calls = Arc::default();
        let seen = calls.clone();
        let run: Runner = Box::new(move |args, input| {
            seen.lock()
                .unwrap()
                .push((args.to_vec(), input.map(<[u8]>::to_vec)));
            let account = args.last().cloned().unwrap_or_default();
            let mut items = items.lock().unwrap();
            let reply = |success, stdout: &str| {
                Ok(Reply {
                    success,
                    stdout: stdout.to_owned(),
                    stderr: String::new(),
                })
            };
            match args[0].as_str() {
                "lookup" => match items.get(&account) {
                    Some(value) => reply(true, value),
                    None => reply(false, ""),
                },
                "store" => {
                    let value = String::from_utf8(input.unwrap().to_vec()).unwrap();
                    items.insert(account, value);
                    reply(true, "")
                }
                "clear" => {
                    items.remove(&account);
                    reply(true, "")
                }
                other => panic!("unexpected command {other}"),
            }
        });
        (SecretTool::with_runner("nullradix", run), calls)
    }

    #[test]
    fn keeps_replaces_and_deletes_secrets() {
        let (tool, _) = fake();
        assert_eq!(tool.get("a").unwrap(), None);
        tool.put("a", "one").unwrap();
        tool.put("a", "two").unwrap();
        assert_eq!(tool.get("a").unwrap().as_deref(), Some("two"));
        tool.delete("a").unwrap();
        tool.delete("a").unwrap();
        assert_eq!(tool.get("a").unwrap(), None);
    }

    #[test]
    fn secrets_travel_on_stdin_and_never_on_the_command_line() {
        let (tool, calls) = fake();
        tool.put("account-token/1", "sk-ant-oat01-fake").unwrap();
        tool.get("account-token/1").unwrap();
        let calls = calls.lock().unwrap();
        let (args, input) = &calls[0];
        assert_eq!(
            args,
            &[
                "store",
                "--label=nullradix account-token/1",
                "service",
                "nullradix",
                "account",
                "account-token/1"
            ]
        );
        assert_eq!(input.as_deref(), Some(b"sk-ant-oat01-fake".as_slice()));
        assert!(
            calls
                .iter()
                .all(|(args, _)| args.iter().all(|arg| !arg.contains("sk-ant")))
        );
    }

    #[test]
    fn a_locked_or_missing_service_is_an_error_not_a_missing_secret() {
        let run: Runner = Box::new(|_, _| {
            Ok(Reply {
                success: false,
                stdout: String::new(),
                stderr: String::from("Cannot autolaunch D-Bus without X11 $DISPLAY\n"),
            })
        });
        let tool = SecretTool::with_runner("nullradix", run);
        assert!(matches!(tool.get("a"), Err(SecretError::Backend { .. })));
        assert!(tool.put("a", "x").is_err());
        assert!(tool.delete("a").is_err());
    }

    #[test]
    fn a_missing_program_is_an_error() {
        let run: Runner = Box::new(|_, _| Err(io::Error::from(io::ErrorKind::NotFound)));
        let tool = SecretTool::with_runner("nullradix", run);
        assert!(tool.get("a").is_err());
    }

    #[test]
    #[ignore = "needs secret-tool and an unlocked Secret Service session"]
    fn talks_to_a_real_secret_service() {
        let service = format!("nullradix-test-{}", std::process::id());
        let tool = SecretTool::detect(&service).unwrap();
        tool.put("a", "sk-ant-oat01-fake").unwrap();
        assert_eq!(tool.get("a").unwrap().as_deref(), Some("sk-ant-oat01-fake"));
        tool.delete("a").unwrap();
        assert_eq!(tool.get("a").unwrap(), None);
    }
}
