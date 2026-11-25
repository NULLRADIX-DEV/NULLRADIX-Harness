use axum::Json;
use axum::extract::State;
use nullradix_app::sessions::{SessionLine, SessionQuery};
use nullradix_domain::{DisplayState, SessionClient, SessionState, SubagentState, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::tickets::{TicketPath, TicketStatusDto};
use crate::{ApiError, ApiPath, ApiQuery, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SessionStateDto {
    Idle,
    Running,
    Waiting,
    Ended,
}

impl From<SessionState> for SessionStateDto {
    fn from(state: SessionState) -> Self {
        match state {
            SessionState::Idle => Self::Idle,
            SessionState::Running => Self::Running,
            SessionState::Waiting => Self::Waiting,
            SessionState::Ended => Self::Ended,
        }
    }
}

/// The state, or `stale` after 30 minutes without a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayStateDto {
    Idle,
    Running,
    Waiting,
    Ended,
    Stale,
}

impl From<DisplayState> for DisplayStateDto {
    fn from(state: DisplayState) -> Self {
        match state {
            DisplayState::Idle => Self::Idle,
            DisplayState::Running => Self::Running,
            DisplayState::Waiting => Self::Waiting,
            DisplayState::Ended => Self::Ended,
            DisplayState::Stale => Self::Stale,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SessionClientDto {
    Nullradix,
    ClaudeCode,
    ClaudeDesktop,
    Unknown,
}

impl From<SessionClient> for SessionClientDto {
    fn from(client: SessionClient) -> Self {
        match client {
            SessionClient::Nullradix => Self::Nullradix,
            SessionClient::ClaudeCode => Self::ClaudeCode,
            SessionClient::ClaudeDesktop => Self::ClaudeDesktop,
            SessionClient::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SubagentStateDto {
    Running,
    Ended,
}

impl From<SubagentState> for SubagentStateDto {
    fn from(state: SubagentState) -> Self {
        match state {
            SubagentState::Running => Self::Running,
            SubagentState::Ended => Self::Ended,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubagentDto {
    pub id: String,
    /// Agent type such as `Explore`.
    #[serde(rename = "type")]
    pub kind: String,
    pub state: SubagentStateDto,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionTicketDto {
    pub key: String,
    pub title: String,
    pub status: TicketStatusDto,
}

/// A Claude Code session as its hooks reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    /// Claude Code's session id.
    pub id: String,
    /// Repository whose checkout holds the cwd; null outside every known checkout.
    pub repository_id: Option<Uuid>,
    pub runner_id: Option<Uuid>,
    /// Per repository, once the repository is known.
    pub number: Option<u32>,
    /// Name to show, such as `SE-001: Fix the login`; may be empty.
    pub label: String,
    pub title: Option<String>,
    pub cwd: String,
    pub state: SessionStateDto,
    pub display_state: DisplayStateDto,
    /// Idle, running or waiting and not stale.
    pub listening: bool,
    pub started_at: i64,
    pub last_event_at: i64,
    pub ended_at: Option<i64>,
    pub runtime_ms: i64,
    pub model: Option<String>,
    pub transcript_path: Option<String>,
    pub client: SessionClientDto,
    /// Run that started the session, for NULLRADIX sessions.
    pub run_name: Option<String>,
    pub subagents: Vec<SubagentDto>,
    /// Linked tickets ordered by key.
    pub tickets: Vec<SessionTicketDto>,
    pub version: i64,
}

impl From<SessionLine> for SessionDto {
    fn from(line: SessionLine) -> Self {
        let label = line.session.label();
        let record = line.session.record();
        Self {
            id: record.id.as_str().to_owned(),
            repository_id: record.repository.map(|id| id.as_uuid()),
            runner_id: record.runner.map(|id| id.as_uuid()),
            number: record.number,
            label,
            title: record.title.clone(),
            cwd: record.cwd.clone(),
            state: record.state.into(),
            display_state: line.view.display_state.into(),
            listening: line.view.listening,
            started_at: record.started_at.as_millis(),
            last_event_at: record.last_event_at.as_millis(),
            ended_at: record.ended_at.map(Timestamp::as_millis),
            runtime_ms: i64::try_from(line.view.runtime.as_millis()).unwrap_or(i64::MAX),
            model: record.model.clone(),
            transcript_path: record.transcript_path.clone(),
            client: record.client.into(),
            run_name: record.run_name.clone(),
            subagents: record
                .subagents
                .iter()
                .map(|agent| SubagentDto {
                    id: agent.id.clone(),
                    kind: agent.kind.clone(),
                    state: agent.state.into(),
                    started_at: agent.started_at.as_millis(),
                    ended_at: agent.ended_at.map(Timestamp::as_millis),
                })
                .collect(),
            tickets: line
                .tickets
                .into_iter()
                .map(|ticket| SessionTicketDto {
                    key: ticket.key.to_string(),
                    title: ticket.title,
                    status: ticket.status.into(),
                })
                .collect(),
            version: record.version.value(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SessionList {
    /// Latest activity first.
    pub items: Vec<SessionDto>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct SessionsQuery {
    /// Only sessions in this repository.
    repository_id: Option<Uuid>,
    /// `idle`, `running`, `waiting`, `ended` or `stale`.
    state: Option<String>,
    /// Only sessions linked to this ticket key.
    ticket: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct SessionPath {
    /// Claude Code's session id.
    pub(crate) session_id: String,
}

fn list(lines: Vec<SessionLine>) -> SessionList {
    SessionList {
        items: lines.into_iter().map(SessionDto::from).collect(),
    }
}

#[utoipa::path(
    get,
    path = "/sessions",
    tag = "sessions",
    params(SessionsQuery),
    responses(
        (status = 200, body = SessionList),
        (status = 401, body = Problem),
        (status = 404, description = "The ticket does not exist", body = Problem),
        (status = 422, description = "Invalid state or ticket key", body = Problem),
    )
)]
pub(crate) async fn list_sessions(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<SessionsQuery>,
) -> Result<Json<SessionList>, ApiError> {
    let lines = state
        .sessions
        .list(SessionQuery {
            repository: query.repository_id.map(nullradix_domain::Id::from_uuid),
            state: query.state.as_deref(),
            ticket: query.ticket.as_deref(),
        })
        .await?;
    Ok(Json(list(lines)))
}

#[utoipa::path(
    get,
    path = "/sessions/{sessionId}",
    tag = "sessions",
    params(SessionPath),
    responses(
        (status = 200, body = SessionDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn get_session(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<SessionPath>,
) -> Result<Json<SessionDto>, ApiError> {
    Ok(Json(state.sessions.get(&path.session_id).await?.into()))
}

#[utoipa::path(
    get,
    path = "/tickets/{ticketKey}/sessions",
    tag = "tickets",
    params(TicketPath),
    responses(
        (status = 200, body = SessionList),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn list_ticket_sessions(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
) -> Result<Json<SessionList>, ApiError> {
    Ok(Json(list(
        state.sessions.for_ticket(&path.ticket_key).await?,
    )))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{Value, json};

    use crate::tests::{TestApi, bearer, send, test_api};

    fn cwd() -> &'static str {
        if cfg!(windows) { "C:\\repo" } else { "/repo" }
    }

    /// An hour ago plus `offset` milliseconds: old enough to be stale, young enough to list.
    fn at(offset: i64) -> i64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        i64::try_from(now.as_millis()).unwrap() - 3_600_000 + offset
    }

    async fn hook(api: &TestApi, kind: &str, body: Value) {
        let mut event = json!({
            "id": uuid::Uuid::now_v7().to_string(),
            "sessionId": "s-1",
            "cwd": cwd(),
            "occurredAt": at(0),
        });
        if let (Some(event), Some(extra)) = (event.as_object_mut(), body.as_object()) {
            event.extend(extra.clone());
        }
        let request = bearer(Request::post(format!("/api/v1/hooks/{kind}")), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(event.to_string()))
            .unwrap();
        assert_eq!(send(&api.router, request).await.0, 200);
    }

    async fn get(api: &TestApi, path: &str) -> (u16, Value) {
        let request = bearer(Request::get(path), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    #[tokio::test]
    async fn hooks_build_sessions_that_list_and_show() {
        let api = test_api().await;
        hook(&api, "session-start", json!({ "model": "opus" })).await;
        hook(
            &api,
            "user-prompt",
            json!({ "occurredAt": at(1_000), "prompt": "Fix the login" }),
        )
        .await;
        hook(
            &api,
            "subagent-start",
            json!({ "occurredAt": at(2_000), "agentId": "a1", "agentType": "Explore" }),
        )
        .await;
        let (status, list) = get(&api, "/api/v1/sessions").await;
        assert_eq!(status, 200);
        let session = &list["items"][0];
        assert_eq!(session["id"], "s-1");
        assert_eq!(session["state"], "running");
        assert_eq!(session["displayState"], "stale");
        assert_eq!(session["title"], "Fix the login");
        assert_eq!(session["model"], "opus");
        assert_eq!(session["subagents"][0]["type"], "Explore");
        assert_eq!(session["repositoryId"], Value::Null);

        let (status, one) = get(&api, "/api/v1/sessions/s-1").await;
        assert_eq!((status, &one["id"]), (200, &json!("s-1")));
        let (status, running) = get(&api, "/api/v1/sessions?state=running").await;
        assert_eq!(status, 200);
        assert_eq!(running["items"], json!([]));
    }

    #[tokio::test]
    async fn unknown_sessions_tickets_and_states_are_refused() {
        let api = test_api().await;
        let (status, problem) = get(&api, "/api/v1/sessions/nope").await;
        assert_eq!(
            (status, &problem["code"]),
            (404, &json!("session-not-found"))
        );
        let (status, problem) = get(&api, "/api/v1/sessions?state=busy").await;
        assert_eq!((status, &problem["code"]), (422, &json!("state-invalid")));
        let (status, problem) = get(&api, "/api/v1/tickets/ARC-1/sessions").await;
        assert_eq!(
            (status, &problem["code"]),
            (404, &json!("ticket-not-found"))
        );
        let (status, problem) = get(&api, "/api/v1/sessions?ticket=x").await;
        assert_eq!(status, 422, "{problem}");
    }
}
