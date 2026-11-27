use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Extension, State};
use nullradix_app::AppError;
use nullradix_app::auth::Principal;
use nullradix_app::hooks::{HookReply, invalid};
use nullradix_domain::{HookDetails, HookEvent, HookKind, Id, SessionRef, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::auth::require_token;
use crate::{ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

const DEFAULT_LIST: u32 = 50;
const MAX_LIST: u32 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HookKindDto {
    SessionStart,
    UserPrompt,
    Notification,
    Stop,
    SubagentStart,
    SubagentStop,
    SessionEnd,
}

impl HookKindDto {
    /// The name in URLs and on the command line.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        HookKind::from(self).as_str()
    }

    /// Whether Claude Code reads context from the answer to this hook.
    #[must_use]
    pub fn answers(self) -> bool {
        HookKind::from(self).answers()
    }
}

impl From<HookKindDto> for HookKind {
    fn from(kind: HookKindDto) -> Self {
        match kind {
            HookKindDto::SessionStart => Self::SessionStart,
            HookKindDto::UserPrompt => Self::UserPrompt,
            HookKindDto::Notification => Self::Notification,
            HookKindDto::Stop => Self::Stop,
            HookKindDto::SubagentStart => Self::SubagentStart,
            HookKindDto::SubagentStop => Self::SubagentStop,
            HookKindDto::SessionEnd => Self::SessionEnd,
        }
    }
}

impl From<HookKind> for HookKindDto {
    fn from(kind: HookKind) -> Self {
        match kind {
            HookKind::SessionStart => Self::SessionStart,
            HookKind::UserPrompt => Self::UserPrompt,
            HookKind::Notification => Self::Notification,
            HookKind::Stop => Self::Stop,
            HookKind::SubagentStart => Self::SubagentStart,
            HookKind::SubagentStop => Self::SubagentStop,
            HookKind::SessionEnd => Self::SessionEnd,
        }
    }
}

/// One hook report; unknown fields are ignored because hooks skip the version handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HookEventDto {
    /// Chosen by the reporter, so a resend is recognised.
    pub id: Uuid,
    pub session_id: String,
    /// Absolute working directory of the session.
    pub cwd: String,
    /// Unix milliseconds.
    pub occurred_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// Ticket key the session was started for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    /// Run that started the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Which Claude client ran the hook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
}

impl HookEventDto {
    /// The event with long texts cut as the daemon cuts them, so it stays small on the wire.
    #[must_use]
    pub fn clipped(mut self) -> Self {
        let details = self.take_details().clipped();
        self.put_details(details);
        self
    }

    /// The domain event, refused when the session or cwd is invalid.
    pub fn into_event(mut self, kind: HookKindDto) -> Result<HookEvent, AppError> {
        let session = SessionRef::parse(&self.session_id).map_err(|error| invalid(&error))?;
        let details = self.take_details();
        HookEvent::new(
            Id::from_uuid(self.id),
            kind.into(),
            session,
            &self.cwd,
            Timestamp::from_millis(self.occurred_at),
            details,
        )
        .map_err(|error| invalid(&error))
    }

    fn take_details(&mut self) -> HookDetails {
        HookDetails {
            transcript_path: self.transcript_path.take(),
            prompt: self.prompt.take(),
            source: self.source.take(),
            model: self.model.take(),
            reason: self.reason.take(),
            message: self.message.take(),
            notification_type: self.notification_type.take(),
            agent_id: self.agent_id.take(),
            agent_type: self.agent_type.take(),
            ticket: self.ticket.take(),
            run: self.run.take(),
            entrypoint: self.entrypoint.take(),
        }
    }

    fn put_details(&mut self, details: HookDetails) {
        self.transcript_path = details.transcript_path;
        self.prompt = details.prompt;
        self.source = details.source;
        self.model = details.model;
        self.reason = details.reason;
        self.message = details.message;
        self.notification_type = details.notification_type;
        self.agent_id = details.agent_id;
        self.agent_type = details.agent_type;
        self.ticket = details.ticket;
        self.run = details.run;
        self.entrypoint = details.entrypoint;
    }
}

impl From<HookEvent> for HookEventDto {
    fn from(event: HookEvent) -> Self {
        let mut dto = Self {
            id: event.id().as_uuid(),
            session_id: event.session().as_str().to_owned(),
            cwd: event.cwd().to_owned(),
            occurred_at: event.occurred_at().as_millis(),
            transcript_path: None,
            prompt: None,
            source: None,
            model: None,
            reason: None,
            message: None,
            notification_type: None,
            agent_id: None,
            agent_type: None,
            ticket: None,
            run: None,
            entrypoint: None,
        };
        dto.put_details(event.details().clone());
        dto
    }
}

/// What the hook passes on to Claude Code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HookReplyDto {
    /// Text Claude Code adds to the conversation.
    pub additional_context: Option<String>,
    /// Variables for the environment of the session.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Name Claude Code shows for the session, such as `SE-001: Fix the login`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_title: Option<String>,
}

impl From<HookReply> for HookReplyDto {
    fn from(reply: HookReply) -> Self {
        Self {
            additional_context: reply.additional_context,
            env: reply.env,
            session_title: reply.session_title,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RecordedHookDto {
    pub kind: HookKindDto,
    pub event: HookEventDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HookEventList {
    pub items: Vec<RecordedHookDto>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct HookPath {
    /// The hook that fired.
    event: HookKindDto,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ListQuery {
    /// Newest events first by when they happened, at most 500.
    limit: Option<u32>,
}

#[utoipa::path(
    post,
    path = "/hooks/{event}",
    tag = "hooks",
    security(("bearer" = [])),
    params(HookPath),
    request_body = HookEventDto,
    responses(
        (status = 200, body = HookReplyDto),
        (status = 401, body = Problem),
        (status = 400, body = Problem),
        (status = 403, description = "Browser sessions cannot report hooks", body = Problem),
        (status = 422, description = "Invalid event, body or hook name", body = Problem),
    )
)]
pub(crate) async fn receive_hook(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<HookPath>,
    ApiJson(input): ApiJson<HookEventDto>,
) -> Result<Json<HookReplyDto>, ApiError> {
    require_token(&principal, "report hooks")?;
    let event = input.into_event(path.event)?;
    Ok(Json(state.hooks.receive(&event).await?.into()))
}

#[utoipa::path(
    get,
    path = "/hook-events",
    tag = "hooks",
    params(ListQuery),
    responses((status = 200, body = HookEventList), (status = 401, body = Problem))
)]
pub(crate) async fn list_hook_events(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<HookEventList>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIST).clamp(1, MAX_LIST);
    let items = state
        .hooks
        .recent(limit)
        .await?
        .into_iter()
        .map(|event| RecordedHookDto {
            kind: event.kind().into(),
            event: event.into(),
        })
        .collect();
    Ok(Json(HookEventList { items }))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;

    use crate::tests::{bearer, local, open_session, send, test_api};

    fn event(id: &str) -> serde_json::Value {
        json!({
            "id": id,
            "sessionId": "s-1",
            "cwd": if cfg!(windows) { "C:\\repo" } else { "/repo" },
            "occurredAt": 1,
            "prompt": "fix it",
            "futureField": true,
        })
    }

    #[tokio::test]
    async fn token_holders_report_hooks_once_per_event_id() {
        let api = test_api().await;
        let id = uuid::Uuid::now_v7().to_string();
        let first = json!({ "additionalContext": null, "env": {}, "sessionTitle": "fix it" });
        // a resend changes nothing, so it names nothing either
        let again = json!({ "additionalContext": null, "env": {} });
        for expected in [first, again] {
            let request = bearer(Request::post("/api/v1/hooks/user-prompt"), &api.token)
                .header("content-type", "application/json")
                .body(Body::from(event(&id).to_string()))
                .unwrap();
            let (status, _, reply) = send(&api.router, request).await;
            assert_eq!(status, 200);
            assert_eq!(reply, expected);
        }
        let request = bearer(Request::get("/api/v1/hook-events"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, list) = send(&api.router, request).await;
        assert_eq!(status, 200);
        let items = list["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["kind"], "user-prompt");
        assert_eq!(items[0]["event"]["prompt"], "fix it");
    }

    #[tokio::test]
    async fn sessions_cannot_report_and_bad_input_is_refused() {
        let api = test_api().await;
        let cookie = open_session(&api).await;
        let request = local(Request::post("/api/v1/hooks/stop"))
            .header("cookie", &cookie)
            .header("origin", "http://127.0.0.1:4711")
            .header("content-type", "application/json")
            .body(Body::from(
                event(&uuid::Uuid::now_v7().to_string()).to_string(),
            ))
            .unwrap();
        let (status, _, problem) = send(&api.router, request).await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (403, Some("token-required"))
        );

        let mut relative = event(&uuid::Uuid::now_v7().to_string());
        relative["cwd"] = json!("repo");
        let request = bearer(Request::post("/api/v1/hooks/stop"), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(relative.to_string()))
            .unwrap();
        let (status, _, problem) = send(&api.router, request).await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (422, Some("hook-cwd-invalid"))
        );

        let request = bearer(Request::post("/api/v1/hooks/no-such-hook"), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(
                event(&uuid::Uuid::now_v7().to_string()).to_string(),
            ))
            .unwrap();
        let (status, _, _) = send(&api.router, request).await;
        assert_eq!(status, 422);
    }
}
