use std::path::PathBuf;

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nullradix_app::auth::Principal;
use nullradix_app::dispatch::runs::{
    MAX_TRANSCRIPT_LINES, RunDetail, StartDispatchRun, TranscriptEntry,
};
use nullradix_app::runs::RunArtifact;
use nullradix_domain::{
    Abort, AbortCategory, DeniedTool, DispatchOutcome, DispatchRun, Id, ModelUsage, RunKind,
    RunProfile, RunResult, RunTrigger, RunUsage,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::auth::require_token;
use crate::runs::{RunDto, RunPath};
use crate::{API_PREFIX, ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

const DEFAULT_TRANSCRIPT: u32 = 500;

/// What a run does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RunKindDto {
    Work,
    Merge,
    Research,
    Plan,
}

impl From<RunKind> for RunKindDto {
    fn from(kind: RunKind) -> Self {
        match kind {
            RunKind::Work => Self::Work,
            RunKind::Merge => Self::Merge,
            RunKind::Research => Self::Research,
            RunKind::Plan => Self::Plan,
        }
    }
}

impl From<RunKindDto> for RunKind {
    fn from(kind: RunKindDto) -> Self {
        match kind {
            RunKindDto::Work => Self::Work,
            RunKindDto::Merge => Self::Merge,
            RunKindDto::Research => Self::Research,
            RunKindDto::Plan => Self::Plan,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RunTriggerDto {
    Dispatcher,
    Manual,
}

impl From<RunTrigger> for RunTriggerDto {
    fn from(trigger: RunTrigger) -> Self {
        match trigger {
            RunTrigger::Dispatcher => Self::Dispatcher,
            RunTrigger::Manual => Self::Manual,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AbortCategoryDto {
    Timeout,
    MaxTurns,
    Context,
    ExitCode,
    ClaudeError,
    UsageLimit,
    StartFailed,
    Vanished,
}

impl From<AbortCategory> for AbortCategoryDto {
    fn from(category: AbortCategory) -> Self {
        match category {
            AbortCategory::Timeout => Self::Timeout,
            AbortCategory::MaxTurns => Self::MaxTurns,
            AbortCategory::Context => Self::Context,
            AbortCategory::ExitCode => Self::ExitCode,
            AbortCategory::ClaudeError => Self::ClaudeError,
            AbortCategory::UsageLimit => Self::UsageLimit,
            AbortCategory::StartFailed => Self::StartFailed,
            AbortCategory::Vanished => Self::Vanished,
        }
    }
}

/// How the dispatcher settled a finished run; none while it is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DispatchOutcomeDto {
    Done,
    Review,
    Human,
    Abort,
    AbortFinal,
    Blocked,
    Question,
    PauseLimit,
    PauseClaudeMissing,
    SetupFailed,
    Taken,
    Gone,
    Other,
    MergeSession,
    Research,
    Planned,
}

impl From<DispatchOutcome> for DispatchOutcomeDto {
    fn from(outcome: DispatchOutcome) -> Self {
        match outcome {
            DispatchOutcome::Done => Self::Done,
            DispatchOutcome::Review => Self::Review,
            DispatchOutcome::Human => Self::Human,
            DispatchOutcome::Abort => Self::Abort,
            DispatchOutcome::AbortFinal => Self::AbortFinal,
            DispatchOutcome::Blocked => Self::Blocked,
            DispatchOutcome::Question => Self::Question,
            DispatchOutcome::PauseLimit => Self::PauseLimit,
            DispatchOutcome::PauseClaudeMissing => Self::PauseClaudeMissing,
            DispatchOutcome::SetupFailed => Self::SetupFailed,
            DispatchOutcome::Taken => Self::Taken,
            DispatchOutcome::Gone => Self::Gone,
            DispatchOutcome::Other => Self::Other,
            DispatchOutcome::MergeSession => Self::MergeSession,
            DispatchOutcome::Research => Self::Research,
            DispatchOutcome::Planned => Self::Planned,
        }
    }
}

/// The profile a run started with, as it was then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunProfileDto {
    /// Gone when the profile was deleted since.
    pub id: Option<Uuid>,
    pub name: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub max_turns: u32,
    pub timeout_minutes: u32,
}

impl From<RunProfile> for RunProfileDto {
    fn from(profile: RunProfile) -> Self {
        Self {
            id: profile.id,
            name: profile.name,
            model: profile.model,
            effort: profile.effort,
            max_turns: profile.max_turns,
            timeout_minutes: profile.timeout_minutes,
        }
    }
}

/// Purpose and settings of a Claude run of the dispatcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DispatchDto {
    pub kind: RunKindDto,
    pub trigger: RunTriggerDto,
    /// Key of the ticket, or of the question for research; kept when it is deleted.
    pub subject_key: Option<String>,
    pub profile: RunProfileDto,
    pub account_id: Option<Uuid>,
    /// Hash of the dispatcher configuration the run started with.
    pub config_hash: String,
}

impl From<&DispatchRun> for DispatchDto {
    fn from(dispatch: &DispatchRun) -> Self {
        Self {
            kind: dispatch.kind.into(),
            trigger: dispatch.trigger.into(),
            subject_key: dispatch.subject_key.clone(),
            profile: dispatch.profile.clone().into(),
            account_id: dispatch.account,
            config_hash: dispatch.config_hash.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageDto {
    pub model: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
}

impl From<&ModelUsage> for ModelUsageDto {
    fn from(model: &ModelUsage) -> Self {
        Self {
            model: model.model.clone(),
            input_tokens: model.input_tokens,
            output_tokens: model.output_tokens,
            cache_read_tokens: model.cache_read_tokens,
            cache_write_tokens: model.cache_write_tokens,
            cost_usd: model.cost_usd,
        }
    }
}

/// Cost and tokens Claude reported for a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunUsageDto {
    pub cost_usd: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub num_turns: Option<i64>,
    /// The model that cost the most.
    pub model: Option<String>,
    pub models: Vec<ModelUsageDto>,
}

impl From<&RunUsage> for RunUsageDto {
    fn from(usage: &RunUsage) -> Self {
        Self {
            cost_usd: usage.cost_usd,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            num_turns: usage.num_turns,
            model: usage.model.clone(),
            models: usage.models.iter().map(ModelUsageDto::from).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AbortDto {
    pub category: AbortCategoryDto,
    pub reason: String,
}

impl From<&Abort> for AbortDto {
    fn from(abort: &Abort) -> Self {
        Self {
            category: abort.category.into(),
            reason: abort.reason.clone(),
        }
    }
}

/// What the dispatcher read from the output of a finished run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunResultDto {
    pub claude_session_id: Option<String>,
    /// Tool calls Claude Code refused; empty without a result to count them in.
    pub denied_count: Option<u32>,
    pub abort: Option<AbortDto>,
    pub usage: Option<RunUsageDto>,
    pub outcome: Option<DispatchOutcomeDto>,
}

impl From<&RunResult> for RunResultDto {
    fn from(result: &RunResult) -> Self {
        Self {
            claude_session_id: result.session_id.clone(),
            denied_count: result.denied_count,
            abort: result.abort.as_ref().map(AbortDto::from),
            usage: result.usage.as_ref().map(RunUsageDto::from),
            outcome: result.outcome.map(DispatchOutcomeDto::from),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeniedToolDto {
    pub tool: String,
    pub count: u32,
}

impl From<DeniedTool> for DeniedToolDto {
    fn from(tool: DeniedTool) -> Self {
        Self {
            tool: tool.tool,
            count: tool.count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotDto {
    pub id: Uuid,
    pub media_type: String,
    pub bytes: u64,
    /// The tool call that took it.
    pub tool_use_id: Option<String>,
    pub created_at: i64,
}

impl From<RunArtifact> for ScreenshotDto {
    fn from(artifact: RunArtifact) -> Self {
        Self {
            id: artifact.id,
            media_type: artifact.screenshot.media_type,
            bytes: artifact.screenshot.bytes,
            tool_use_id: artifact.screenshot.tool_use_id,
            created_at: artifact.created_at.as_millis(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ScreenshotList {
    pub items: Vec<ScreenshotDto>,
}

/// A run with its prompt, call and the rows that belong to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunDetailDto {
    #[serde(flatten)]
    pub run: RunDto,
    /// Working folder of a Claude run.
    pub cwd: Option<String>,
    /// Arguments after the program, as the run was started.
    pub args: Vec<String>,
    pub prompt: Option<String>,
    pub denied_tools: Vec<DeniedToolDto>,
    pub screenshots: Vec<ScreenshotDto>,
}

impl From<RunDetail> for RunDetailDto {
    fn from(detail: RunDetail) -> Self {
        let dispatch = detail.run.dispatch().cloned();
        Self {
            cwd: dispatch.as_ref().map(|dispatch| dispatch.cwd.clone()),
            args: dispatch
                .as_ref()
                .map_or_else(Vec::new, |dispatch| dispatch.args.clone()),
            prompt: dispatch.map(|dispatch| dispatch.prompt),
            denied_tools: detail.denied.into_iter().map(DeniedToolDto::from).collect(),
            screenshots: detail
                .artifacts
                .into_iter()
                .map(ScreenshotDto::from)
                .collect(),
            run: RunDto::from(detail.run),
        }
    }
}

/// What a log line is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LineKindDto {
    Start,
    Say,
    Read,
    Edit,
    Search,
    Test,
    Run,
    Todo,
    Tool,
    Ok,
    Fail,
    Done,
    Abort,
    Raw,
}

impl From<nullradix_app::dispatch::transcript::LineKind> for LineKindDto {
    fn from(kind: nullradix_app::dispatch::transcript::LineKind) -> Self {
        use nullradix_app::dispatch::transcript::LineKind;
        match kind {
            LineKind::Start => Self::Start,
            LineKind::Say => Self::Say,
            LineKind::Read => Self::Read,
            LineKind::Edit => Self::Edit,
            LineKind::Search => Self::Search,
            LineKind::Test => Self::Test,
            LineKind::Run => Self::Run,
            LineKind::Todo => Self::Todo,
            LineKind::Tool => Self::Tool,
            LineKind::Ok => Self::Ok,
            LineKind::Fail => Self::Fail,
            LineKind::Done => Self::Done,
            LineKind::Abort => Self::Abort,
            LineKind::Raw => Self::Raw,
        }
    }
}

/// One readable line of the run log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptLineDto {
    /// The output line it came from.
    pub seq: i64,
    pub kind: LineKindDto,
    pub text: String,
    /// Id of the tool call, on a call and on its result.
    pub reference: Option<String>,
    /// The exact call or the whole message when the text is cut.
    pub detail: Option<String>,
}

impl From<TranscriptEntry> for TranscriptLineDto {
    fn from(entry: TranscriptEntry) -> Self {
        Self {
            seq: entry.seq,
            kind: entry.line.kind.into(),
            text: entry.line.text,
            reference: entry.line.reference,
            detail: entry.line.detail,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptDto {
    pub lines: Vec<TranscriptLineDto>,
    /// The last output line read; ask for lines after it.
    pub last_seq: i64,
    /// The run ended and no output is left.
    pub finished: bool,
    /// The read stopped at its limit; ask again at once.
    pub more: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct TranscriptQuery {
    /// Read output lines after this sequence number.
    #[serde(default)]
    after: i64,
    /// At most this many output lines, up to 1000.
    limit: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct ScreenshotPath {
    /// Run id.
    id: Uuid,
    /// Screenshot id.
    screenshot: Uuid,
}

#[utoipa::path(
    get,
    path = "/runs/{id}/transcript",
    tag = "runs",
    params(RunPath, TranscriptQuery),
    responses(
        (status = 200, body = TranscriptDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
        (status = 503, body = Problem),
    )
)]
pub(crate) async fn run_transcript(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<RunPath>,
    ApiQuery(query): ApiQuery<TranscriptQuery>,
) -> Result<Json<TranscriptDto>, ApiError> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_TRANSCRIPT)
        .clamp(1, MAX_TRANSCRIPT_LINES);
    let page = state
        .dispatch_runs
        .transcript(Id::from_uuid(path.id), query.after.max(0), limit)
        .await?;
    Ok(Json(TranscriptDto {
        lines: page
            .entries
            .into_iter()
            .map(TranscriptLineDto::from)
            .collect(),
        last_seq: page.last_seq,
        finished: page.finished,
        more: page.more,
    }))
}

#[utoipa::path(
    get,
    path = "/runs/{id}/screenshots",
    tag = "runs",
    params(RunPath),
    responses(
        (status = 200, body = ScreenshotList),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn list_screenshots(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<RunPath>,
) -> Result<Json<ScreenshotList>, ApiError> {
    let detail = state.dispatch_runs.detail(Id::from_uuid(path.id)).await?;
    Ok(Json(ScreenshotList {
        items: detail
            .artifacts
            .into_iter()
            .map(ScreenshotDto::from)
            .collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/runs/{id}/screenshots/{screenshot}",
    tag = "runs",
    params(ScreenshotPath),
    responses(
        (status = 200, description = "The image", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
        (status = 503, body = Problem),
    )
)]
pub(crate) async fn screenshot(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ScreenshotPath>,
) -> Result<Response, ApiError> {
    let (artifact, bytes) = state
        .dispatch_runs
        .screenshot(Id::from_uuid(path.id), path.screenshot)
        .await?;
    Ok(screenshot_response(&artifact.screenshot.media_type, bytes))
}

/// The stored type is the model's word, so only image types a browser shows safely pass.
fn served_type(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "image/png",
        "image/jpeg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

fn screenshot_response(media_type: &str, bytes: Vec<u8>) -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static(served_type(media_type)),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            // an image must not run script on the app origin
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("sandbox; default-src 'none'"),
            ),
        ],
        bytes,
    )
        .into_response()
}

/// A Claude run to start from the dispatcher configuration; the scheduler uses the same use case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartDispatchRunBody {
    pub kind: RunKindDto,
    /// Ticket key, or question key for research runs.
    pub subject_key: String,
    pub prompt: String,
    /// Absolute working directory on the daemon's machine.
    pub cwd: String,
    /// A profile by name; none takes the one of the kind, else the default.
    pub profile: Option<String>,
}

#[utoipa::path(
    post,
    path = "/dispatcher/runs",
    tag = "dispatcher",
    security(("bearer" = [])),
    request_body = StartDispatchRunBody,
    responses(
        (status = 201, body = RunDto,
            headers(("Location" = String, description = "URL of the new run"))),
        (status = 401, body = Problem),
        (status = 403, description = "Browser sessions cannot start runs", body = Problem),
        (status = 404, description = "The ticket or question does not exist", body = Problem),
        (status = 422, body = Problem),
        (status = 503, description = "The runner could not start it", body = Problem),
    )
)]
pub(crate) async fn start_dispatch_run(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiJson(input): ApiJson<StartDispatchRunBody>,
) -> Result<(StatusCode, HeaderMap, Json<RunDto>), ApiError> {
    require_token(&principal, "start runs")?;
    let run = state
        .dispatch_runs
        .start(StartDispatchRun {
            kind: input.kind.into(),
            trigger: RunTrigger::Manual,
            subject: input.subject_key,
            prompt: input.prompt,
            cwd: PathBuf::from(input.cwd),
            profile: input.profile,
            account: None,
            extra_tools: Vec::new(),
            agent: None,
            overrides: nullradix_domain::LabelOverrides::default(),
        })
        .await?;
    let run = RunDto::from(run);
    let mut headers = HeaderMap::new();
    if let Ok(location) = HeaderValue::from_str(&format!("{API_PREFIX}/runs/{}", run.id)) {
        headers.insert(header::LOCATION, location);
    }
    Ok((StatusCode::CREATED, headers, Json(run)))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{Value, json};

    use super::screenshot_response;
    use crate::tests::{TestApi, bearer, local, send, test_api};

    async fn get(api: &TestApi, path: &str) -> (u16, Value) {
        let request = bearer(Request::get(path), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, body) = send(&api.router, request).await;
        (status.as_u16(), body)
    }

    async fn start(api: &TestApi, body: &Value) -> (u16, Value) {
        let request = bearer(Request::post("/api/v1/dispatcher/runs"), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, body) = send(&api.router, request).await;
        (status.as_u16(), body)
    }

    async fn add_ticket(api: &TestApi) {
        let project = api.projects.create("ARC", "Arches").await.unwrap();
        let board = api
            .boards
            .create(project.id(), "Main", &api.person)
            .await
            .unwrap()
            .board;
        let path = format!(
            "/api/v1/projects/{}/boards/{}/tickets",
            project.id().as_uuid(),
            board.id().as_uuid()
        );
        let request = bearer(Request::post(path), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(json!({ "title": "Fix it" }).to_string()))
            .unwrap();
        assert_eq!(send(&api.router, request).await.0, 201);
    }

    #[tokio::test]
    async fn run_lists_are_narrowed_by_key_and_refuse_bad_filters() {
        let api = test_api().await;
        let (status, list) = get(&api, "/api/v1/runs?ticketKey=arc-1&kind=work").await;
        assert_eq!((status, list["items"].clone()), (200, json!([])));
        let (status, list) = get(&api, "/api/v1/runs?questionKey=ARC-Q0001").await;
        assert_eq!((status, list["items"].clone()), (200, json!([])));
        for (query, code) in [
            ("ticketKey=..%2Fx", "ticket-id-invalid"),
            ("ticketKey=", "ticket-id-invalid"),
            ("ticketKey=ARC-Q0001", "ticket-id-invalid"),
            ("questionKey=ARC-0001", "question-id-invalid"),
            (
                "ticketKey=ARC-0001&questionKey=ARC-Q0001",
                "run-filter-invalid",
            ),
        ] {
            let (status, problem) = get(&api, &format!("/api/v1/runs?{query}")).await;
            assert_eq!(
                (status, problem["code"].as_str()),
                (422, Some(code)),
                "{query}"
            );
        }
        let (status, _) = get(&api, "/api/v1/runs?kind=nonsense").await;
        assert_eq!(status, 422);
    }

    #[test]
    fn screenshots_are_served_with_an_allowed_type_and_no_sniffing() {
        for (stored, served) in [
            ("image/png", "image/png"),
            ("image/webp", "image/webp"),
            ("text/html", "application/octet-stream"),
            ("image/svg+xml", "application/octet-stream"),
            ("image/png\r\nX-Evil: 1", "application/octet-stream"),
        ] {
            let response = screenshot_response(stored, vec![1]);
            let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok());
            assert_eq!(header("content-type"), Some(served), "{stored:?}");
            assert_eq!(header("x-content-type-options"), Some("nosniff"));
            assert_eq!(
                header("content-security-policy"),
                Some("sandbox; default-src 'none'")
            );
        }
    }

    #[tokio::test]
    async fn unknown_runs_have_no_transcript_or_screenshots() {
        let api = test_api().await;
        let id = uuid::Uuid::now_v7();
        for path in [
            format!("/api/v1/runs/{id}/transcript"),
            format!("/api/v1/runs/{id}/screenshots"),
            format!("/api/v1/runs/{id}/screenshots/{}", uuid::Uuid::now_v7()),
        ] {
            let (status, problem) = get(&api, &path).await;
            assert_eq!(
                (status, problem["code"].as_str()),
                (404, Some("run-not-found")),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn only_token_holders_start_dispatch_runs_and_the_subject_must_exist() {
        let api = test_api().await;
        let cwd = std::env::temp_dir();
        let body = json!({
            "kind": "work", "subjectKey": "ARC-0001", "prompt": "Go.", "cwd": cwd
        });
        let code = api.auth.create_handoff().unwrap();
        let session = api.auth.redeem_handoff(&code).await.unwrap();
        let request = local(Request::post("/api/v1/dispatcher/runs"))
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

        let (status, problem) = start(&api, &body).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (404, Some("ticket-not-found"))
        );
        let (status, problem) = start(
            &api,
            &json!({ "kind": "work", "subjectKey": "x", "prompt": "Go.", "cwd": cwd }),
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("ticket-id-invalid"))
        );
        let (status, problem) = start(
            &api,
            &json!({ "kind": "work", "subjectKey": "ARC-0001", "prompt": " ", "cwd": cwd }),
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("run-prompt-invalid"))
        );
    }

    #[tokio::test]
    async fn a_run_the_runner_cannot_start_is_recorded_with_its_settings_and_abort() {
        let api = test_api().await;
        add_ticket(&api).await;
        let cwd = std::env::temp_dir();
        let body = json!({
            "kind": "work", "subjectKey": "arc-1", "prompt": "Go.", "cwd": cwd, "profile": "small"
        });
        // the test runner has no supervisor to start
        let (status, problem) = start(&api, &body).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (503, Some("runner-failed"))
        );
        let (status, list) = get(&api, "/api/v1/runs?ticketKey=ARC-0001").await;
        assert_eq!(status, 200);
        let run = &list["items"][0];
        assert_eq!(run["status"], "failed");
        assert_eq!(run["label"], "work ARC-0001");
        assert_eq!(run["dispatch"]["kind"], "work");
        assert_eq!(run["dispatch"]["trigger"], "manual");
        assert_eq!(run["dispatch"]["subjectKey"], "ARC-0001");
        assert_eq!(run["dispatch"]["profile"]["name"], "small");
        assert_eq!(run["dispatch"]["profile"]["maxTurns"], 40);
        assert_eq!(run["result"]["abort"]["category"], "start-failed");

        let id = run["id"].as_str().unwrap();
        let (status, detail) = get(&api, &format!("/api/v1/runs/{id}")).await;
        assert_eq!(status, 200);
        assert_eq!(detail["status"], "failed");
        assert_eq!(detail["prompt"], "Go.");
        assert_eq!(detail["args"][0], "-p");
        assert_eq!(detail["deniedTools"], json!([]));
        assert_eq!(detail["screenshots"], json!([]));
        let (status, page) = get(&api, &format!("/api/v1/runs/{id}/transcript")).await;
        assert_eq!(status, 200);
        assert_eq!(
            (
                page["lines"].clone(),
                page["finished"].clone(),
                page["more"].clone()
            ),
            (json!([]), json!(true), json!(false))
        );
    }
}
