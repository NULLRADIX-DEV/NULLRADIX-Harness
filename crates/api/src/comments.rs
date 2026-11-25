use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use nullradix_app::auth::Principal;
use nullradix_app::comments::{Author, CommentLine};
use nullradix_domain::{ActorKind, SystemMarker};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::tickets::TicketPath;
use crate::{ApiError, ApiJson, ApiPath, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ActorKindDto {
    Human,
    Session,
    Subagent,
    Dispatcher,
    System,
}

impl From<ActorKind> for ActorKindDto {
    fn from(kind: ActorKind) -> Self {
        match kind {
            ActorKind::Human => Self::Human,
            ActorKind::Session => Self::Session,
            ActorKind::Subagent => Self::Subagent,
            ActorKind::Dispatcher => Self::Dispatcher,
            ActorKind::System => Self::System,
        }
    }
}

/// What a dispatcher comment reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SystemMarkerDto {
    Start,
    Abort,
    Pause,
    Question,
    Merged,
    MergeConflict,
    MergeWait,
    Merge,
    Setup,
    ResearchEmpty,
    PlanEmpty,
    Review,
    ReviewReturned,
}

impl From<SystemMarker> for SystemMarkerDto {
    fn from(marker: SystemMarker) -> Self {
        match marker {
            SystemMarker::Start => Self::Start,
            SystemMarker::Abort => Self::Abort,
            SystemMarker::Pause => Self::Pause,
            SystemMarker::Question => Self::Question,
            SystemMarker::Merged => Self::Merged,
            SystemMarker::MergeConflict => Self::MergeConflict,
            SystemMarker::MergeWait => Self::MergeWait,
            SystemMarker::Merge => Self::Merge,
            SystemMarker::Setup => Self::Setup,
            SystemMarker::ResearchEmpty => Self::ResearchEmpty,
            SystemMarker::PlanEmpty => Self::PlanEmpty,
            SystemMarker::Review => Self::Review,
            SystemMarker::ReviewReturned => Self::ReviewReturned,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommentDto {
    pub id: Uuid,
    pub ticket_key: String,
    pub author_id: Uuid,
    pub author_name: String,
    pub author_kind: ActorKindDto,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_marker: Option<SystemMarkerDto>,
    pub created_at: i64,
}

impl From<CommentLine> for CommentDto {
    fn from(line: CommentLine) -> Self {
        let record = line.comment.record();
        Self {
            id: record.id.as_uuid(),
            ticket_key: line.ticket_key.to_string(),
            author_id: record.author.as_uuid(),
            author_name: line.author_name,
            author_kind: line.author_kind.into(),
            body: record.body.as_str().to_owned(),
            system_marker: record.system_marker.map(SystemMarkerDto::from),
            created_at: record.created_at.as_millis(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CommentList {
    /// Oldest first.
    pub items: Vec<CommentDto>,
}

/// A new comment; leading and trailing whitespace is dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddComment {
    pub body: String,
    /// Name to write under, such as `claude`; the acting person when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Agent session of the caller: the comment counts as delivered and names a session author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

#[utoipa::path(
    get,
    path = "/tickets/{ticketKey}/comments",
    tag = "tickets",
    params(TicketPath),
    responses(
        (status = 200, body = CommentList),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn list_comments(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
) -> Result<Json<CommentList>, ApiError> {
    let items = state
        .comments
        .list(&path.ticket_key)
        .await?
        .into_iter()
        .map(CommentDto::from)
        .collect();
    Ok(Json(CommentList { items }))
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/comments",
    tag = "tickets",
    params(TicketPath),
    request_body = AddComment,
    responses(
        (status = 201, body = CommentDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, description = "Empty body or an invalid author name", body = Problem),
    )
)]
pub(crate) async fn add_comment(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<AddComment>,
) -> Result<(StatusCode, Json<CommentDto>), ApiError> {
    let author = Author {
        by_person: matches!(principal, Principal::Session { .. }),
        name: input.author.as_deref(),
        session: input.session.as_deref(),
    };
    let line = state
        .comments
        .add(&path.ticket_key, &input.body, author, &state.person)
        .await?;
    Ok((StatusCode::CREATED, Json(line.into())))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{Value, json};

    use crate::tests::{TestApi, bearer, local, open_session, send, test_api};

    async fn with_ticket() -> TestApi {
        let api = test_api().await;
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
        let request = bearer(Request::post(path.as_str()), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(json!({ "title": "Talk" }).to_string()))
            .unwrap();
        assert_eq!(send(&api.router, request).await.0, 201);
        api
    }

    async fn post(api: &TestApi, path: &str, body: &Value) -> (u16, Value) {
        let request = bearer(Request::post(path), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    async fn get(api: &TestApi, path: &str) -> (u16, Value) {
        let request = bearer(Request::get(path), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    #[tokio::test]
    async fn comments_are_added_and_listed_oldest_first() {
        let api = with_ticket().await;
        let (status, comment) = post(
            &api,
            "/api/v1/tickets/arc-1/comments",
            &json!({ "body": " Step 2 done ", "author": "claude", "session": "s-1" }),
        )
        .await;
        assert_eq!(status, 201);
        assert_eq!(comment["ticketKey"], "ARC-0001");
        assert_eq!(comment["body"], "Step 2 done");
        assert_eq!(comment["authorName"], "claude");
        assert_eq!(comment["authorKind"], "session");
        assert!(comment.get("systemMarker").is_none());
        post(
            &api,
            "/api/v1/tickets/ARC-1/comments",
            &json!({ "body": "Next" }),
        )
        .await;

        let (status, list) = get(&api, "/api/v1/tickets/ARC-1/comments").await;
        assert_eq!(status, 200);
        let bodies: Vec<&str> = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["body"].as_str())
            .collect();
        assert_eq!(bodies, ["Step 2 done", "Next"]);
        assert_eq!(list["items"][1]["authorName"], "tester");
        let (_, detail) = get(&api, "/api/v1/tickets/ARC-1").await;
        assert_eq!(detail["comments"], list["items"]);
        assert_eq!(detail["ticket"]["version"], 3);
        assert_eq!(detail["history"].as_array().map(Vec::len), Some(1));
    }

    #[tokio::test]
    async fn empty_comments_and_unknown_tickets_are_refused() {
        let api = with_ticket().await;
        let (status, problem) = post(
            &api,
            "/api/v1/tickets/ARC-1/comments",
            &json!({ "body": " \n\t" }),
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("comment-empty"))
        );
        let (status, problem) = post(
            &api,
            "/api/v1/tickets/ARC-9/comments",
            &json!({ "body": "x" }),
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (404, Some("ticket-not-found"))
        );
        let (status, problem) = get(&api, "/api/v1/tickets/ARC-9/comments").await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (404, Some("ticket-not-found"))
        );
    }

    #[tokio::test]
    async fn a_browser_session_comments_as_the_person() {
        let api = with_ticket().await;
        let cookie = open_session(&api).await;
        let request = local(Request::post("/api/v1/tickets/ARC-1/comments"))
            .header(axum::http::header::COOKIE, &cookie)
            .header(axum::http::header::ORIGIN, "http://127.0.0.1:4711")
            .header("content-type", "application/json")
            .body(Body::from(json!({ "body": "Looks good" }).to_string()))
            .unwrap();
        let (status, _, comment) = send(&api.router, request).await;
        assert_eq!(status, 201);
        assert_eq!(
            (
                comment["authorName"].as_str(),
                comment["authorKind"].as_str()
            ),
            (Some("tester"), Some("human"))
        );
    }
}
