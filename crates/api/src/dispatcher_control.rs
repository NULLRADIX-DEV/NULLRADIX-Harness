use axum::Json;
use axum::extract::State;
use nullradix_app::AppError;
use nullradix_app::dispatch::control::{DispatcherStatus, PauseFor};
use nullradix_app::dispatch::pass::PassReport;
use nullradix_domain::{DispatcherPhase, Id, Run, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::dispatch_runs::RunKindDto;
use crate::{ApiError, ApiJson, ApiQuery, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DispatcherPhaseDto {
    Off,
    Idle,
    Running,
    Paused,
    OutsideWindow,
}

impl From<DispatcherPhase> for DispatcherPhaseDto {
    fn from(phase: DispatcherPhase) -> Self {
        match phase {
            DispatcherPhase::Off => Self::Off,
            DispatcherPhase::Idle => Self::Idle,
            DispatcherPhase::Running => Self::Running,
            DispatcherPhase::Paused => Self::Paused,
            DispatcherPhase::OutsideWindow => Self::OutsideWindow,
        }
    }
}

/// What a pass did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PassReportDto {
    /// Unix milliseconds the pass began.
    pub at: i64,
    /// Why nothing new could start, in words.
    pub idle: Option<String>,
    pub started: Vec<String>,
    /// Questions whose research started.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub research: Vec<String>,
    /// Ready tickets that wait for a free place.
    pub queued: Vec<String>,
    pub released: Vec<String>,
    pub errors: Vec<String>,
}

impl From<PassReport> for PassReportDto {
    fn from(report: PassReport) -> Self {
        Self {
            at: report.at,
            idle: report.idle,
            started: report.started,
            research: report.research,
            queued: report.queued,
            released: report.released,
            errors: report.errors,
        }
    }
}

/// A run of the dispatcher that has not ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherRunDto {
    pub id: Uuid,
    pub kind: RunKindDto,
    /// Ticket or question key.
    pub subject: Option<String>,
    pub profile: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Unix milliseconds.
    pub started_at: Option<i64>,
}

/// A planned ticket and why the dispatcher does or does not take it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TicketReadinessDto {
    pub key: String,
    pub title: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherStatusDto {
    pub enabled: bool,
    pub state: DispatcherPhaseDto,
    pub message: String,
    /// Unix milliseconds the pause ends.
    pub paused_until: Option<i64>,
    /// Tickets worked on at the same time at most.
    pub wip: i64,
    pub active: u32,
    pub planning: u32,
    pub runs: Vec<DispatcherRunDto>,
    pub tickets: Vec<TicketReadinessDto>,
    pub last_pass_at: Option<i64>,
    pub last_errors: Vec<String>,
    pub next_pass_at: Option<i64>,
    pub agent_prefix: String,
    pub board_id: Option<Uuid>,
    pub version: i64,
}

fn run_line(run: &Run) -> Option<DispatcherRunDto> {
    let dispatch = run.dispatch()?;
    Some(DispatcherRunDto {
        id: run.id().as_uuid(),
        kind: dispatch.kind.into(),
        subject: dispatch.subject_key.clone(),
        profile: dispatch.profile.name.clone(),
        model: dispatch.profile.model.clone(),
        effort: dispatch.profile.effort.clone(),
        started_at: run.started_at().map(Timestamp::as_millis),
    })
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

impl DispatcherStatusDto {
    /// The state as the wire spells it.
    #[must_use]
    pub const fn state_word(&self) -> &'static str {
        match self.state {
            DispatcherPhaseDto::Off => "off",
            DispatcherPhaseDto::Idle => "idle",
            DispatcherPhaseDto::Running => "running",
            DispatcherPhaseDto::Paused => "paused",
            DispatcherPhaseDto::OutsideWindow => "outside-window",
        }
    }
}

impl From<DispatcherStatus> for DispatcherStatusDto {
    fn from(status: DispatcherStatus) -> Self {
        Self {
            enabled: status.enabled,
            state: status.phase.into(),
            message: status.message,
            paused_until: status.paused_until.map(Timestamp::as_millis),
            wip: status.wip,
            active: count(status.active),
            planning: count(status.planning),
            runs: status.runs.iter().filter_map(run_line).collect(),
            tickets: status
                .tickets
                .into_iter()
                .map(|row| TicketReadinessDto {
                    key: row.key,
                    title: row.title,
                    reason: row.reason,
                })
                .collect(),
            last_pass_at: status.last_pass_at.map(Timestamp::as_millis),
            last_errors: status.last_errors,
            next_pass_at: status.next_pass_at.map(Timestamp::as_millis),
            agent_prefix: status.agent_prefix,
            board_id: status.board.as_ref().map(Id::as_uuid),
            version: status.version.value(),
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusQuery {
    /// Shows the readiness of the tickets of this board only.
    board_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDispatcher {
    /// Start new tickets, or not, whatever the configuration says.
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PauseDispatcher {
    /// Pause for this many minutes, 1 to 1440.
    pub minutes: Option<u32>,
    /// Pause until this moment in Unix milliseconds; it must lie in the future.
    pub until: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/dispatcher",
    tag = "dispatcher",
    params(StatusQuery),
    responses(
        (status = 200, body = DispatcherStatusDto),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown board", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn get_dispatcher(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<StatusQuery>,
) -> Result<Json<DispatcherStatusDto>, ApiError> {
    let board = query.board_id.map(Id::from_uuid);
    Ok(Json(state.dispatcher.status(board).await?.into()))
}

#[utoipa::path(
    patch,
    path = "/dispatcher",
    tag = "dispatcher",
    request_body = UpdateDispatcher,
    responses((status = 200, body = DispatcherStatusDto), (status = 401, body = Problem), (status = 422, body = Problem))
)]
pub(crate) async fn update_dispatcher(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<UpdateDispatcher>,
) -> Result<Json<DispatcherStatusDto>, ApiError> {
    Ok(Json(
        state.dispatcher.set_enabled(input.enabled).await?.into(),
    ))
}

#[utoipa::path(
    post,
    path = "/dispatcher/pause",
    tag = "dispatcher",
    request_body = PauseDispatcher,
    responses((status = 200, body = DispatcherStatusDto), (status = 401, body = Problem), (status = 422, body = Problem))
)]
pub(crate) async fn pause_dispatcher(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<PauseDispatcher>,
) -> Result<Json<DispatcherStatusDto>, ApiError> {
    let how = match (input.minutes, input.until) {
        (Some(minutes), None) => PauseFor::Minutes(minutes),
        (None, Some(until)) => PauseFor::Until(Timestamp::from_millis(until)),
        _ => {
            return Err(AppError::invalid(
                "dispatcher-pause-invalid",
                "give either minutes or until",
            )
            .into());
        }
    };
    Ok(Json(state.dispatcher.pause(how).await?.into()))
}

#[utoipa::path(
    post,
    path = "/dispatcher/resume",
    tag = "dispatcher",
    responses((status = 200, body = DispatcherStatusDto), (status = 401, body = Problem))
)]
pub(crate) async fn resume_dispatcher(
    State(state): State<ApiState>,
) -> Result<Json<DispatcherStatusDto>, ApiError> {
    Ok(Json(state.dispatcher.resume().await?.into()))
}

#[utoipa::path(
    post,
    path = "/dispatcher/passes",
    tag = "dispatcher",
    responses(
        (status = 200, description = "What the pass did", body = PassReportDto),
        (status = 401, body = Problem),
    )
)]
pub(crate) async fn run_pass(
    State(state): State<ApiState>,
) -> Result<Json<PassReportDto>, ApiError> {
    Ok(Json(state.dispatcher.run_pass().await.into()))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;

    use crate::tests::{TestApi, bearer, local, send, test_api};

    async fn call(
        api: &TestApi,
        request: axum::http::request::Builder,
        body: Option<serde_json::Value>,
    ) -> (u16, serde_json::Value) {
        let request = bearer(request, &api.token);
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    #[tokio::test]
    async fn a_fresh_dispatcher_is_off_and_idle() {
        let api = test_api().await;
        let (status, body) = call(&api, local(Request::get("/api/v1/dispatcher")), None).await;
        assert_eq!(status, 200);
        assert_eq!(body["enabled"], false);
        assert_eq!(body["state"], "off");
        assert_eq!(body["wip"], 1);
        assert_eq!(body["tickets"], json!([]));
        assert_eq!(body["lastPassAt"], json!(null));
    }

    #[tokio::test]
    async fn the_switch_and_the_pause_change_the_state_and_are_validated() {
        let api = test_api().await;
        let patch = || local(Request::patch("/api/v1/dispatcher"));
        let (status, body) = call(&api, patch(), Some(json!({ "enabled": true }))).await;
        assert_eq!((status, &body["state"]), (200, &json!("idle")));
        let (_, body) = call(
            &api,
            local(Request::post("/api/v1/dispatcher/pause")),
            Some(json!({ "minutes": 15 })),
        )
        .await;
        assert_eq!(body["state"], "paused");
        assert!(body["pausedUntil"].as_i64().unwrap() > 0);
        let (_, body) = call(
            &api,
            local(Request::post("/api/v1/dispatcher/resume")),
            None,
        )
        .await;
        assert_eq!(body["state"], "idle");
        for bad in [
            json!({ "minutes": 0 }),
            json!({ "minutes": 1441 }),
            json!({ "minutes": 5, "until": 1 }),
            json!({}),
            json!({ "until": 1 }),
        ] {
            let (status, body) = call(
                &api,
                local(Request::post("/api/v1/dispatcher/pause")),
                Some(bad),
            )
            .await;
            assert_eq!(status, 422);
            assert_eq!(body["code"], "dispatcher-pause-invalid");
        }
    }

    #[tokio::test]
    async fn a_pass_runs_on_request() {
        let api = test_api().await;
        let (status, body) = call(
            &api,
            local(Request::post("/api/v1/dispatcher/passes")),
            Some(json!({})),
        )
        .await;
        assert_eq!(status, 200);
        assert!(body["at"].as_i64().is_some());
        assert_eq!(body["started"], json!([]));
    }
}
