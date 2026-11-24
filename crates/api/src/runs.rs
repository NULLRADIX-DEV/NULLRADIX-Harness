use std::path::PathBuf;
use std::time::Duration;

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use nullradix_app::AppError;
use nullradix_app::auth::Principal;
use nullradix_app::runs::{OutputLine, OutputStream, RunSpec};
use nullradix_domain::{Id, Run, RunStatus, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{API_PREFIX, ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

const DEFAULT_LIST: u32 = 50;
const MAX_LIST: u32 = 500;
const MAX_OUTPUT: u32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RunStatusDto {
    Starting,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Lost,
}

impl From<RunStatus> for RunStatusDto {
    fn from(status: RunStatus) -> Self {
        match status {
            RunStatus::Starting => Self::Starting,
            RunStatus::Running => Self::Running,
            RunStatus::Succeeded => Self::Succeeded,
            RunStatus::Failed => Self::Failed,
            RunStatus::Cancelled => Self::Cancelled,
            RunStatus::TimedOut => Self::TimedOut,
            RunStatus::Lost => Self::Lost,
        }
    }
}

/// A run as the API shows it; timestamps are Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunDto {
    pub id: Uuid,
    pub label: String,
    pub status: RunStatusDto,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub version: i64,
}

impl From<Run> for RunDto {
    fn from(run: Run) -> Self {
        Self {
            id: run.id().as_uuid(),
            label: run.label().as_str().to_owned(),
            status: run.status().into(),
            exit_code: run.exit_code(),
            error: run.error().map(str::to_owned),
            created_at: run.created_at().as_millis(),
            started_at: run.started_at().map(Timestamp::as_millis),
            finished_at: run.finished_at().map(Timestamp::as_millis),
            version: run.version().value(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunList {
    pub items: Vec<RunDto>,
}

/// A command to run on the daemon's machine; only token holders may start runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRun {
    pub label: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Absolute working directory on the daemon's machine.
    pub cwd: String,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OutputStreamDto {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OutputLineDto {
    pub seq: i64,
    pub stream: OutputStreamDto,
    pub text: String,
}

impl From<OutputLine> for OutputLineDto {
    fn from(line: OutputLine) -> Self {
        Self {
            seq: line.seq,
            stream: match line.stream {
                OutputStream::Stdout => OutputStreamDto::Stdout,
                OutputStream::Stderr => OutputStreamDto::Stderr,
            },
            text: line.text,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunOutput {
    pub lines: Vec<OutputLineDto>,
    /// Whether the run ended and these are its last lines; then no lines will follow.
    pub finished: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct RunPath {
    /// Run id.
    id: Uuid,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ListQuery {
    /// Newest runs first, at most 500.
    limit: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct OutputQuery {
    /// Return lines after this sequence number.
    #[serde(default)]
    after: i64,
    /// At most this many lines, up to 1000.
    limit: Option<u32>,
}

#[utoipa::path(
    get,
    path = "/runs",
    tag = "runs",
    params(ListQuery),
    responses((status = 200, body = RunList), (status = 401, body = Problem))
)]
pub(crate) async fn list_runs(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<RunList>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIST).clamp(1, MAX_LIST);
    let items = state
        .runs
        .list(limit)
        .await?
        .into_iter()
        .map(RunDto::from)
        .collect();
    Ok(Json(RunList { items }))
}

#[utoipa::path(
    post,
    path = "/runs",
    tag = "runs",
    security(("bearer" = [])),
    request_body = StartRun,
    responses(
        (status = 201, body = RunDto,
            headers(("Location" = String, description = "URL of the new run"))),
        (status = 401, body = Problem),
        (status = 403, description = "Browser sessions cannot start runs", body = Problem),
        (status = 422, body = Problem),
        (status = 503, description = "The runner could not start it", body = Problem),
    )
)]
pub(crate) async fn start_run(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiJson(input): ApiJson<StartRun>,
) -> Result<(StatusCode, HeaderMap, Json<RunDto>), ApiError> {
    if !matches!(principal, Principal::Token { .. }) {
        return Err(
            AppError::forbidden("token-required", "only the local CLI may start runs").into(),
        );
    }
    let spec = RunSpec {
        program: input.program,
        args: input.args,
        cwd: PathBuf::from(input.cwd),
        timeout: input.timeout_seconds.map(Duration::from_secs),
    };
    let run = RunDto::from(state.runs.start(&input.label, &spec).await?);
    let mut headers = HeaderMap::new();
    if let Ok(location) = HeaderValue::from_str(&format!("{API_PREFIX}/runs/{}", run.id)) {
        headers.insert(header::LOCATION, location);
    }
    Ok((StatusCode::CREATED, headers, Json(run)))
}

#[utoipa::path(
    get,
    path = "/runs/{id}",
    tag = "runs",
    params(RunPath),
    responses(
        (status = 200, body = RunDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn get_run(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<RunPath>,
) -> Result<Json<RunDto>, ApiError> {
    Ok(Json(state.runs.get(Id::from_uuid(path.id)).await?.into()))
}

#[utoipa::path(
    post,
    path = "/runs/{id}/cancel",
    tag = "runs",
    params(RunPath),
    responses(
        (status = 202, description = "The run was asked to stop", body = RunDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "The run already finished", body = Problem),
    )
)]
pub(crate) async fn cancel_run(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<RunPath>,
) -> Result<(StatusCode, Json<RunDto>), ApiError> {
    let run = state.runs.cancel(Id::from_uuid(path.id)).await?;
    Ok((StatusCode::ACCEPTED, Json(run.into())))
}

#[utoipa::path(
    get,
    path = "/runs/{id}/output",
    tag = "runs",
    params(RunPath, OutputQuery),
    responses(
        (status = 200, body = RunOutput),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 503, body = Problem),
    )
)]
pub(crate) async fn run_output(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<RunPath>,
    ApiQuery(query): ApiQuery<OutputQuery>,
) -> Result<Json<RunOutput>, ApiError> {
    let limit = query.limit.unwrap_or(MAX_OUTPUT).clamp(1, MAX_OUTPUT);
    let (run, progress) = state
        .runs
        .output(Id::from_uuid(path.id), query.after, limit)
        .await?;
    // a lost run has no report but will not write more either
    let ended = progress.report.is_some() || !run.status().is_active();
    let finished = ended && u32::try_from(progress.lines.len()).is_ok_and(|n| n < limit);
    Ok(Json(RunOutput {
        lines: progress
            .lines
            .into_iter()
            .map(OutputLineDto::from)
            .collect(),
        finished,
    }))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;

    use crate::tests::{bearer, local, send, test_api};

    #[tokio::test]
    async fn only_token_holders_start_runs_and_failures_are_recorded() {
        let api = test_api().await;
        let cwd = std::env::temp_dir();
        let body = json!({ "label": "echo", "program": "echo", "cwd": cwd });

        let code = api.auth.create_handoff().unwrap();
        let session = api.auth.redeem_handoff(&code).await.unwrap();
        let request = local(Request::post("/api/v1/runs"))
            .header("cookie", format!("nullradix_session={}", session.expose()))
            .header("origin", "http://127.0.0.1:4711")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, problem) = send(&api.router, request).await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (403, Some("token-required"))
        );

        let request = bearer(Request::post("/api/v1/runs"), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, problem) = send(&api.router, request).await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (503, Some("runner-failed"))
        );

        let request = bearer(Request::get("/api/v1/runs"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, list) = send(&api.router, request).await;
        assert_eq!(status, 200);
        assert_eq!(list["items"][0]["status"], "failed");
        assert_eq!(list["items"][0]["label"], "echo");

        // a run that ended without a report still tells followers to stop
        let id = list["items"][0]["id"].as_str().unwrap();
        let request = bearer(
            Request::get(format!("/api/v1/runs/{id}/output")),
            &api.token,
        )
        .body(Body::empty())
        .unwrap();
        let (status, _, output) = send(&api.router, request).await;
        assert_eq!(status, 200);
        assert_eq!(output["finished"], true);
    }

    #[tokio::test]
    async fn rejects_input_the_runner_cannot_run_without_a_record() {
        let api = test_api().await;
        let cwd = std::env::temp_dir().join("nullradix-no-such-dir");
        let body = json!({ "label": "x", "program": "echo", "cwd": cwd });
        let request = bearer(Request::post("/api/v1/runs"), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, problem) = send(&api.router, request).await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (422, Some("invalid-run-cwd"))
        );
        let request = bearer(Request::get("/api/v1/runs"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (_, _, list) = send(&api.router, request).await;
        assert_eq!(list["items"], json!([]));
    }

    #[tokio::test]
    async fn unknown_runs_are_not_found() {
        let api = test_api().await;
        let id = uuid::Uuid::now_v7();
        for path in [
            format!("/api/v1/runs/{id}"),
            format!("/api/v1/runs/{id}/output"),
        ] {
            let request = bearer(Request::get(path), &api.token)
                .body(Body::empty())
                .unwrap();
            let (status, _, problem) = send(&api.router, request).await;
            assert_eq!(
                (status.as_u16(), problem["code"].as_str()),
                (404, Some("run-not-found"))
            );
        }
        let request = bearer(
            Request::post(format!("/api/v1/runs/{id}/cancel")),
            &api.token,
        )
        .body(Body::empty())
        .unwrap();
        let (status, _, _) = send(&api.router, request).await;
        assert_eq!(status, 404);
    }
}
