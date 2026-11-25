//! Asks the usage endpoint of Anthropic through `curl`, which every supported system ships.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::secrets::SecretValue;
use nullradix_app::usage::{UsageApi, UsageReply, UsageWindow, request_failed};
use nullradix_domain::Timestamp;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

/// Endpoint of the subscription limits; it is not documented and may change.
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const REQUEST_LIMIT: Duration = Duration::from_secs(10);
/// Grace for starting curl and its own timeout to fire first.
const PROCESS_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct CurlUsageApi {
    url: String,
}

impl CurlUsageApi {
    #[must_use]
    pub fn anthropic() -> Self {
        Self::at(USAGE_URL)
    }

    #[must_use]
    pub fn at(url: &str) -> Self {
        Self {
            url: url.to_owned(),
        }
    }

    /// The request as a curl config, so the token never appears on a command line.
    fn config(&self, token: &str) -> Option<String> {
        let quoted = |text: &str| -> Option<String> {
            if text.chars().any(char::is_control) {
                return None;
            }
            Some(text.replace('\\', "\\\\").replace('"', "\\\""))
        };
        Some(format!(
            "url = \"{}\"\nheader = \"Authorization: Bearer {}\"\nheader = \"anthropic-beta: oauth-2025-04-20\"\nheader = \"User-Agent: ai-harness\"\n",
            quoted(&self.url)?,
            quoted(token)?
        ))
    }
}

fn failed(detail: &str) -> AppError {
    request_failed(std::io::Error::other(detail.to_owned()))
}

#[async_trait]
impl UsageApi for CurlUsageApi {
    async fn fetch(&self, token: &SecretValue) -> Result<UsageReply, AppError> {
        let config = self
            .config(token.expose())
            .ok_or_else(|| failed("the token holds control characters"))?;
        let mut command = tokio::process::Command::new("curl");
        command
            .args(["--silent", "--show-error", "--include", "--max-time"])
            .arg(REQUEST_LIMIT.as_secs().to_string())
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
        let mut child = command.spawn().map_err(request_failed)?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(config.as_bytes())
                .await
                .map_err(request_failed)?;
        }
        let output = tokio::time::timeout(REQUEST_LIMIT + PROCESS_GRACE, child.wait_with_output())
            .await
            .map_err(|_| failed("curl did not finish in time"))?
            .map_err(request_failed)?;
        if !output.status.success() {
            let said = String::from_utf8_lossy(&output.stderr);
            return Err(failed(said.trim()));
        }
        parse_response(&String::from_utf8_lossy(&output.stdout))
            .ok_or_else(|| failed("curl printed no HTTP response"))
    }
}

/// Status, `Retry-After` and the limit windows of a response printed with `--include`.
fn parse_response(text: &str) -> Option<UsageReply> {
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
        let document: Option<Value> = serde_json::from_str(body).ok();
        let window = |name: &str| {
            document
                .as_ref()
                .and_then(|json| read_window(json.get(name)))
        };
        return Some(UsageReply {
            status,
            five_hour: window("five_hour"),
            seven_day: window("seven_day"),
            retry_after,
        });
    }
}

/// A limit window; one without a usable utilization stays empty.
fn read_window(raw: Option<&Value>) -> Option<UsageWindow> {
    let raw = raw?;
    let utilization = raw.get("utilization").and_then(Value::as_f64)?;
    if !utilization.is_finite() {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is rounded and clamped into range first"
    )]
    let percent = utilization.round().clamp(0.0, f64::from(u32::MAX)) as u32;
    Some(UsageWindow {
        percent,
        resets_at: raw
            .get("resets_at")
            .and_then(Value::as_str)
            .and_then(Timestamp::parse_rfc3339),
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    const TOKEN: &str = "sk-ant-oat01-fake-access-token";

    /// Answers one request with `response` and reports the request text.
    fn serve(response: &'static str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api/oauth/usage", listener.local_addr().unwrap());
        let (sent, received) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
            }
            let _ = sent.send(String::from_utf8_lossy(&request).into_owned());
            stream.write_all(response.as_bytes()).unwrap();
        });
        (url, received)
    }

    fn json_response(status: &str, extra: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
            body.len()
        )
    }

    fn leaked(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }

    #[tokio::test]
    async fn sends_the_token_in_headers_and_reads_both_windows() {
        let body = r#"{"five_hour":{"utilization":42.6,"resets_at":"2026-11-25T18:00:00.000000+00:00"},"seven_day":{"utilization":7,"resets_at":null}}"#;
        let (url, request) = serve(leaked(json_response("200 OK", "", body)));
        let reply = CurlUsageApi::at(&url)
            .fetch(&SecretValue::new(TOKEN))
            .await
            .unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(
            reply.five_hour,
            Some(UsageWindow {
                percent: 43,
                resets_at: Timestamp::parse_rfc3339("2026-11-25T18:00:00Z"),
            })
        );
        assert_eq!(
            reply.seven_day,
            Some(UsageWindow {
                percent: 7,
                resets_at: None
            })
        );
        let request = request.recv().unwrap().to_lowercase();
        assert!(request.starts_with("get /api/oauth/usage"));
        assert!(request.contains(&format!("authorization: bearer {}", TOKEN.to_lowercase())));
        assert!(request.contains("anthropic-beta: oauth-2025-04-20"));
        assert!(request.contains("user-agent: ai-harness"));
    }

    #[tokio::test]
    async fn reports_rate_limits_with_their_retry_after() {
        let (url, _) = serve(leaked(json_response(
            "429 Too Many Requests",
            "Retry-After: 120\r\n",
            "{}",
        )));
        let reply = CurlUsageApi::at(&url)
            .fetch(&SecretValue::new(TOKEN))
            .await
            .unwrap();
        assert_eq!(reply.status, 429);
        assert_eq!(reply.retry_after.as_deref(), Some("120"));
        assert_eq!(reply.five_hour, None);
    }

    #[tokio::test]
    async fn keeps_error_statuses_and_ignores_unreadable_bodies() {
        let (url, _) = serve(leaked(json_response("401 Unauthorized", "", "not json")));
        let reply = CurlUsageApi::at(&url)
            .fetch(&SecretValue::new(TOKEN))
            .await
            .unwrap();
        assert_eq!(
            (reply.status, reply.five_hour, reply.seven_day),
            (401, None, None)
        );
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_is_a_failed_request_without_the_token() {
        let address = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let error = CurlUsageApi::at(&format!("http://{address}/usage"))
            .fetch(&SecretValue::new(TOKEN))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "usage-request-failed");
        assert!(!format!("{error:?}").contains(TOKEN));
    }

    #[tokio::test]
    async fn a_token_with_control_characters_is_never_sent() {
        let api = CurlUsageApi::at("http://127.0.0.1:9/usage");
        let error = api
            .fetch(&SecretValue::new("sk-ant-oat01-a\nurl = \"http://evil\""))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "usage-request-failed");
    }

    #[test]
    fn quotes_tokens_for_the_curl_config() {
        let api = CurlUsageApi::at("https://example.com/usage");
        let config = api.config("a\"b\\c").unwrap();
        assert!(config.contains("Bearer a\\\"b\\\\c\""));
        assert!(config.starts_with("url = \"https://example.com/usage\"\n"));
        assert_eq!(api.config("tab\there"), None);
    }

    #[test]
    fn parses_curl_output_with_interim_answers_and_bare_newlines() {
        let text = "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nRETRY-AFTER: 5\r\n\r\n{\"five_hour\":{\"utilization\":-3}}";
        let reply = parse_response(text).unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.retry_after.as_deref(), Some("5"));
        assert_eq!(reply.five_hour.map(|window| window.percent), Some(0));
        let bare = parse_response("HTTP/2 204 \n\n").unwrap();
        assert_eq!(bare.status, 204);
        assert_eq!(parse_response("garbage"), None);
        assert_eq!(parse_response("HTTP/1.1 abc\r\n\r\n"), None);
    }

    #[test]
    fn windows_without_a_finite_utilization_stay_empty() {
        assert_eq!(read_window(None), None);
        let raw: Value = serde_json::from_str(r#"{"resets_at":"2026-11-25T18:00:00Z"}"#).unwrap();
        assert_eq!(read_window(Some(&raw)), None);
        let big: Value = serde_json::from_str(r#"{"utilization":1e30}"#).unwrap();
        assert_eq!(
            read_window(Some(&big)).map(|window| window.percent),
            Some(u32::MAX)
        );
    }
}
