use std::collections::BTreeMap;
use std::io::Write;
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde_json::{Map, Value, json};
use ureq::Agent;

use crate::daemon::{Binaries, Daemon};
use crate::normalize::Normalizer;
use crate::scenario::{Action, Auth, Cli, File, Http, Scenario, Wait};

const WAIT_POLL: Duration = Duration::from_millis(50);
/// How long one request may take, and how long a read is retried when the connection drops.
const REQUEST_LIMIT: Duration = Duration::from_secs(60);
/// Run states that never change again.
const FINAL: &[&str] = &["succeeded", "failed", "cancelled", "timed-out", "lost"];

/// Plays a scenario against a fresh daemon and returns what happened, normalized.
pub(crate) fn play(scenario: &Scenario, binaries: &Binaries) -> anyhow::Result<Vec<Value>> {
    let daemon = Daemon::start(binaries, &scenario.env)?;
    let mut normalizer = Normalizer::new(
        &[
            ("WORK", &daemon.work),
            ("HOME", daemon.home.path()),
            ("FAKE_CLAUDE", &binaries.fake_claude),
        ],
        &[
            ("ADDRESS", daemon.address.to_string()),
            ("VERSION", String::from(env!("CARGO_PKG_VERSION"))),
        ],
    );
    let agent: Agent = Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(REQUEST_LIMIT))
        .proxy(None)
        .build()
        .into();
    let mut saved = BTreeMap::from([
        (
            String::from("WORK"),
            daemon.work.to_string_lossy().into_owned(),
        ),
        (
            String::from("FAKE_CLAUDE"),
            binaries.fake_claude.to_string_lossy().into_owned(),
        ),
    ]);
    let mut recording = Vec::new();
    for step in &scenario.steps {
        let context = || format!("step \"{}\"", step.name);
        let (mut entry, result) = match &step.action {
            Action::Http(http) => http_step(&agent, &daemon, http, &saved).with_context(context)?,
            Action::Cli(cli) => cli_step(&daemon, cli, &saved).with_context(context)?,
            Action::Wait(wait) => wait_step(&agent, &daemon, wait, &saved).with_context(context)?,
            Action::File(file) => file_step(file, &saved).with_context(context)?,
        };
        for (name, source) in &step.save {
            let value = pick(&result, source)
                .with_context(|| format!("step \"{}\" has no {source} to save", step.name))?;
            saved.insert(name.clone(), value);
        }
        normalizer.value(&mut entry);
        let mut named = json!({ "name": step.name });
        if let (Some(target), Some(fields)) = (named.as_object_mut(), entry.as_object()) {
            target.extend(fields.clone());
        }
        recording.push(named);
    }
    Ok(recording)
}

/// Replaces `{{name}}` with saved values, in text and inside JSON.
fn fill(text: &str, saved: &BTreeMap<String, String>) -> String {
    let mut text = text.to_owned();
    for (name, value) in saved {
        text = text.replace(&format!("{{{{{name}}}}}"), value);
    }
    text
}

fn fill_value(value: &Value, saved: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(text) => Value::from(fill(text, saved)),
        Value::Array(items) => items.iter().map(|item| fill_value(item, saved)).collect(),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, field)| (key.clone(), fill_value(field, saved)))
            .collect(),
        other => other.clone(),
    }
}

/// The value at a dotted path such as `items.0.id`.
fn field<'a>(value: &'a Value, dotted: &str) -> Option<&'a Value> {
    dotted
        .split('.')
        .try_fold(value, |current, key| match key.parse::<usize>() {
            Ok(index) => current.get(index),
            Err(_) => current.get(key),
        })
}

/// `json.<path>` of an HTTP response, or the first line of a command's stdout.
fn pick(result: &Value, source: &str) -> Option<String> {
    let value = if source == "stdout" {
        return result["stdout"]
            .as_str()
            .and_then(|text| text.lines().next())
            .map(|line| line.trim().to_owned());
    } else {
        field(&result["json"], source.strip_prefix("json.")?)?
    };
    Some(match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    })
}

fn http_step(
    agent: &Agent,
    daemon: &Daemon,
    http: &Http,
    saved: &BTreeMap<String, String>,
) -> anyhow::Result<(Value, Value)> {
    let path = fill(&http.path, saved);
    let body = http.body.as_ref().map(|body| fill_value(body, saved));
    let reply = request(agent, daemon, &http.method, &path, body.as_ref(), http.auth)?;
    let mut request = Map::new();
    request.insert(String::from("method"), Value::from(http.method.clone()));
    request.insert(String::from("path"), Value::from(path));
    if let Some(body) = body {
        request.insert(String::from("body"), body);
    }
    let mut response = Map::new();
    response.insert(String::from("status"), Value::from(reply.status));
    if let Some(location) = reply.location {
        response.insert(String::from("location"), Value::from(location));
    }
    let mut result = json!({});
    match reply.payload {
        Payload::Json(json) => {
            result = json!({ "json": json.clone() });
            response.insert(String::from("json"), keep_fields(json, &http.keep));
        }
        Payload::Text(text) => {
            response.insert(String::from("text"), Value::from(text));
        }
        Payload::Empty => {}
    }
    Ok((json!({ "request": request, "response": response }), result))
}

/// Only the dotted paths in `keep`, nested as they were; everything when `keep` is empty.
fn keep_fields(json: Value, keep: &[String]) -> Value {
    if keep.is_empty() {
        return json;
    }
    let mut kept = json!({});
    for path in keep {
        let Some(found) = field(&json, path) else {
            continue;
        };
        let mut target = &mut kept;
        let mut keys = path.split('.').peekable();
        while let Some(key) = keys.next() {
            let Some(map) = target.as_object_mut() else {
                break;
            };
            if keys.peek().is_none() {
                map.insert(key.to_owned(), found.clone());
                break;
            }
            target = map.entry(key.to_owned()).or_insert_with(|| json!({}));
        }
    }
    kept
}

enum Payload {
    Json(Value),
    Text(String),
    Empty,
}

struct Reply {
    status: u16,
    location: Option<String>,
    payload: Payload,
}

fn request(
    agent: &Agent,
    daemon: &Daemon,
    method: &str,
    path: &str,
    body: Option<&Value>,
    auth: Auth,
) -> anyhow::Result<Reply> {
    let url = daemon.url(path);
    let bearer = (auth == Auth::Token).then(|| format!("Bearer {}", daemon.token));
    let mut response = match method {
        "GET" => get_retrying(agent, &url, bearer.as_deref())?,
        "DELETE" => {
            let mut builder = agent.delete(&url);
            if let Some(bearer) = &bearer {
                builder = builder.header("Authorization", bearer);
            }
            builder
                .call()
                .map_err(|error| not_retried(method, &error))?
        }
        "POST" | "PATCH" | "PUT" => {
            let mut builder = match method {
                "POST" => agent.post(&url),
                "PATCH" => agent.patch(&url),
                _ => agent.put(&url),
            };
            if let Some(bearer) = &bearer {
                builder = builder.header("Authorization", bearer);
            }
            match body {
                Some(body) => builder.send_json(body),
                None => builder.send_empty(),
            }
            .map_err(|error| not_retried(method, &error))?
        }
        other => bail!("unsupported method {other}"),
    };
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let location = header("location");
    let json = header("content-type").is_some_and(|kind| kind.contains("json"));
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string()?;
    let payload = if text.is_empty() {
        Payload::Empty
    } else if json {
        Payload::Json(serde_json::from_str(&text)?)
    } else {
        Payload::Text(text)
    };
    Ok(Reply {
        status,
        location,
        payload,
    })
}

/// A GET changes nothing: a refused or reset connection is tried again until the limit.
fn get_retrying(
    agent: &Agent,
    url: &str,
    bearer: Option<&str>,
) -> anyhow::Result<ureq::http::Response<ureq::Body>> {
    let deadline = Instant::now() + REQUEST_LIMIT;
    loop {
        let mut builder = agent.get(url);
        if let Some(bearer) = bearer {
            builder = builder.header("Authorization", bearer);
        }
        match builder.call() {
            Ok(response) => return Ok(response),
            Err(error) if is_transient(&error) && Instant::now() < deadline => {
                thread::sleep(WAIT_POLL);
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Whether the connection was refused or dropped before any answer.
fn is_transient(error: &ureq::Error) -> bool {
    use std::io::ErrorKind;
    match error {
        ureq::Error::ConnectionFailed | ureq::Error::Timeout(ureq::Timeout::Connect) => true,
        ureq::Error::Io(io) => matches!(
            io.kind(),
            ErrorKind::ConnectionReset
                | ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionAborted
                | ErrorKind::BrokenPipe
                | ErrorKind::UnexpectedEof
        ),
        _ => false,
    }
}

/// Writes are never repeated, since the daemon may have applied them before the connection broke.
fn not_retried(method: &str, error: &ureq::Error) -> anyhow::Error {
    if is_transient(error) {
        anyhow::anyhow!("{method} lost its connection ({error}); a write is not sent twice")
    } else {
        anyhow::anyhow!("{method} failed: {error}")
    }
}

fn cli_step(
    daemon: &Daemon,
    cli: &Cli,
    saved: &BTreeMap<String, String>,
) -> anyhow::Result<(Value, Value)> {
    let args: Vec<String> = cli.args.iter().map(|arg| fill(arg, saved)).collect();
    let mut command = daemon.cli();
    let folder = cli
        .cwd
        .as_ref()
        .map_or_else(|| daemon.work.clone(), |dir| fill(dir, saved).into());
    command
        .args(&args)
        .current_dir(folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("cannot start the CLI")?;
    let input = match &cli.stdin {
        Some(Value::String(text)) => fill(text, saved),
        Some(value) => fill_value(value, saved).to_string(),
        None => String::new(),
    };
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let entry = json!({
        "cli": args,
        "exitCode": output.status.code(),
        "stdout": stdout,
        "stderr": stderr,
    });
    Ok((entry, json!({ "stdout": stdout })))
}

/// Writes the lines with saved values filled in; the file holds them each on its own line.
fn file_step(file: &File, saved: &BTreeMap<String, String>) -> anyhow::Result<(Value, Value)> {
    let path = fill(&file.path, saved);
    let mut text = file
        .text
        .as_ref()
        .map_or_else(String::new, |text| fill(text, saved));
    for line in &file.lines {
        text.push_str(&fill_value(line, saved).to_string());
        text.push('\n');
    }
    if let Some(folder) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(folder).with_context(|| format!("cannot create {path}"))?;
    }
    std::fs::write(&path, text).with_context(|| format!("cannot write {path}"))?;
    let entry = json!({ "file": path, "lines": file.lines.len() });
    Ok((entry, json!({})))
}

fn wait_step(
    agent: &Agent,
    daemon: &Daemon,
    wait: &Wait,
    saved: &BTreeMap<String, String>,
) -> anyhow::Result<(Value, Value)> {
    let path = fill(&wait.path, saved);
    let deadline = Instant::now() + Duration::from_millis(wait.timeout_ms);
    loop {
        let reply = request(agent, daemon, "GET", &path, None, Auth::Token)?;
        let seen = match &reply.payload {
            Payload::Json(json) => field(json, &wait.field).cloned(),
            _ => None,
        };
        if seen.as_ref() == Some(&wait.equals)
            && let Payload::Json(json) = reply.payload
        {
            let entry = json!({ "wait": path, "field": wait.field, "reached": wait.equals });
            return Ok((entry, json!({ "json": json })));
        }
        // a run that ended otherwise will not change again
        let ended = wait.field == "status"
            && seen
                .as_ref()
                .and_then(Value::as_str)
                .is_some_and(|status| FINAL.contains(&status));
        if ended || Instant::now() > deadline {
            let last = seen.map_or_else(|| format!("status {}", reply.status), |v| v.to_string());
            bail!(
                "{path} never had {} = {}, last {last}",
                wait.field,
                wait.equals
            );
        }
        thread::sleep(WAIT_POLL);
    }
}
