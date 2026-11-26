//! Runs `curl`, which every supported system ships, with the request as a config on stdin.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Largest answer that is read; a bigger one fails the request.
pub(crate) const RESPONSE_MAX: usize = 1 << 20;
/// Largest part of curl's own complaint that is kept.
const ERROR_MAX: u64 = 4_096;
/// Grace for starting curl and its own timeout to fire first.
const PROCESS_GRACE: Duration = Duration::from_secs(5);

/// `text` as a curl config string; none when it holds control characters.
pub(crate) fn quoted(text: &str) -> Option<String> {
    if text.chars().any(char::is_control) {
        return None;
    }
    Some(text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Runs curl with `config`, so secrets never appear on a command line; the answer includes headers.
pub(crate) async fn run(config: &str, limit: Duration) -> Result<String, String> {
    let mut command = tokio::process::Command::new("curl");
    command
        .args(["--silent", "--show-error", "--include", "--max-time"])
        .arg(limit.as_secs().to_string())
        .args(["--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(config.as_bytes())
            .await
            .map_err(|error| error.to_string())?;
    }
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(String::from("curl has no output"));
    };
    let finished = async {
        let mut body = Vec::new();
        stdout
            .take(RESPONSE_MAX as u64 + 1)
            .read_to_end(&mut body)
            .await
            .map_err(|error| error.to_string())?;
        if body.len() > RESPONSE_MAX {
            return Err(String::from("the answer is too large"));
        }
        let mut said = Vec::new();
        stderr
            .take(ERROR_MAX)
            .read_to_end(&mut said)
            .await
            .map_err(|error| error.to_string())?;
        let status = child.wait().await.map_err(|error| error.to_string())?;
        if status.success() {
            Ok(String::from_utf8_lossy(&body).into_owned())
        } else {
            Err(String::from_utf8_lossy(&said).trim().to_owned())
        }
    };
    tokio::time::timeout(limit + PROCESS_GRACE, finished)
        .await
        .map_err(|_| String::from("curl did not finish in time"))?
}

/// The status line, `Retry-After` and body of the last answer in output printed with `--include`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Response<'a> {
    pub(crate) status: u16,
    pub(crate) retry_after: Option<String>,
    pub(crate) body: &'a str,
}

pub(crate) fn split_response(text: &str) -> Option<Response<'_>> {
    let mut rest = text;
    loop {
        let (head, body) = rest
            .split_once("\r\n\r\n")
            .or_else(|| rest.split_once("\n\n"))?;
        let mut lines = head.lines();
        let status: u16 = lines
            .next()?
            .strip_prefix("HTTP/")?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()?;
        // an interim answer such as 100 Continue comes before the real one
        if (100..200).contains(&status) {
            rest = body;
            continue;
        }
        let retry_after = lines.find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("retry-after")
                .then(|| value.trim().to_owned())
        });
        return Some(Response {
            status,
            retry_after,
            body,
        });
    }
}
