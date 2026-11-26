//! Runs `curl`, which every supported system ships, with the request as a config on stdin.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

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
        .args(arguments(limit))
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
        let (body, said) = read_both(stdout, stderr).await?;
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

/// `-q` must come first to take effect: no `.curlrc` may change the request.
fn arguments(limit: Duration) -> Vec<String> {
    let mut arguments: Vec<String> = [
        "-q",
        "--silent",
        "--show-error",
        "--include",
        "--suppress-connect-headers",
        "--max-time",
    ]
    .map(String::from)
    .into();
    arguments.push(limit.as_secs().to_string());
    arguments.extend(["--config", "-"].map(String::from));
    arguments
}

/// Reads both streams together, so a full pipe on one never stalls curl writing the other.
async fn read_both(
    stdout: impl AsyncRead + Unpin,
    mut stderr: impl AsyncRead + Unpin,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let out = async {
        let mut body = Vec::new();
        stdout
            .take(RESPONSE_MAX as u64 + 1)
            .read_to_end(&mut body)
            .await
            .map_err(|error| error.to_string())?;
        if body.len() > RESPONSE_MAX {
            return Err(String::from("the answer is too large"));
        }
        Ok(body)
    };
    let err = async {
        let mut said = Vec::new();
        (&mut stderr)
            .take(ERROR_MAX)
            .read_to_end(&mut said)
            .await
            .map_err(|error| error.to_string())?;
        // what is beyond the cap is read and dropped
        tokio::io::copy(&mut stderr, &mut tokio::io::sink())
            .await
            .map_err(|error| error.to_string())?;
        Ok(said)
    };
    tokio::try_join!(out, err)
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
        // an interim answer such as 100 Continue, or a proxy's CONNECT reply, comes first
        let connect = status == 200
            && head
                .lines()
                .next()
                .is_some_and(|line| line.to_ascii_lowercase().contains("connection established"));
        if (100..200).contains(&status) || connect {
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

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::*;

    #[test]
    fn the_reply_after_a_proxy_connect_preamble_is_the_answer() {
        let text = "HTTP/1.1 200 Connection established\r\nProxy-Agent: x\r\n\r\nHTTP/2 200\r\nretry-after: 7\r\n\r\n{\"ok\":true}";
        let response = split_response(text).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.retry_after.as_deref(), Some("7"));
        assert_eq!(response.body, "{\"ok\":true}");
        let bare = "HTTP/1.0 200 connection established\n\nHTTP/1.1 429 Too Many\n\nlater";
        assert_eq!(split_response(bare).unwrap().status, 429);
    }

    #[test]
    fn a_plain_ok_is_not_taken_for_a_preamble() {
        let response = split_response("HTTP/1.1 200 OK\r\n\r\nbody").unwrap();
        assert_eq!(response.body, "body");
    }

    #[test]
    fn curl_ignores_curlrc_and_hides_connect_headers() {
        let arguments = arguments(Duration::from_secs(10));
        assert_eq!(arguments[0], "-q");
        assert!(arguments.iter().any(|a| a == "--suppress-connect-headers"));
    }

    #[tokio::test]
    async fn a_large_stderr_does_not_wait_for_stdout_to_end() {
        let (mut out_write, out_read) = tokio::io::duplex(64);
        let (mut err_write, err_read) = tokio::io::duplex(64);
        let reading = tokio::spawn(read_both(out_read, err_read));
        let writing = tokio::spawn(async move {
            err_write.write_all(&vec![b'e'; 100_000]).await.unwrap();
        });
        for _ in 0..20_000 {
            tokio::task::yield_now().await;
        }
        assert!(writing.is_finished(), "stderr was not read meanwhile");
        out_write.write_all(b"body").await.unwrap();
        drop(out_write);
        let (body, said) = reading.await.unwrap().unwrap();
        assert_eq!(body, b"body");
        assert_eq!(said.len() as u64, ERROR_MAX);
    }
}
