//! Favorite sessions: list, mark with a note, unmark.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use nullradix_app::favorites::FavoriteLine;
use nullradix_domain::{DisplayState, session_label};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::sessions::{DisplayStateDto, SessionPath, SessionStateDto};
use crate::{ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResumptionDto {
    pub session_id: String,
    pub at: i64,
}

/// A marked session; its state is the live one while the session still exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteDto {
    pub session_id: String,
    pub repository_id: Option<Uuid>,
    pub note: String,
    /// Ticket keys of the session, or of the snapshot once it is gone.
    pub tickets: Vec<String>,
    pub cwd: String,
    /// Whether the folder still exists, so the command can work.
    pub cwd_exists: bool,
    pub title: Option<String>,
    /// Name to show, such as `SE-001: Fix the login`; may be empty.
    pub label: String,
    pub marked_at: i64,
    pub state: SessionStateDto,
    pub display_state: DisplayStateDto,
    /// Whether the session is still in the session list.
    pub live: bool,
    pub last_event_at: i64,
    /// Later sessions that resumed this one, oldest first.
    pub resumed_as: Vec<ResumptionDto>,
    /// Shell command that resumes the session in its folder.
    pub command: String,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FavoriteList {
    /// Latest marked first.
    pub items: Vec<FavoriteDto>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MarkFavorite {
    /// Replaces the note; left out keeps it. Cut to 2000 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct FavoritesQuery {
    /// Only favorites in this repository.
    repository_id: Option<Uuid>,
}

impl From<FavoriteLine> for FavoriteDto {
    fn from(marked: FavoriteLine) -> Self {
        let command = marked.favorite.command();
        let record = marked.favorite.record();
        let live = marked.live.as_ref();
        let tickets = match live {
            Some(live) => live
                .tickets
                .iter()
                .map(|ticket| ticket.key.to_string())
                .collect(),
            None => record.tickets.iter().map(ToString::to_string).collect(),
        };
        let title = live
            .and_then(|live| live.session.record().title.clone())
            .or_else(|| record.title.clone());
        let label = live.map_or_else(
            || session_label(None, title.as_deref()),
            |live| live.session.label(),
        );
        let (state, display_state, last_event_at) = match live {
            Some(live) => (
                live.session.record().state,
                live.view.display_state,
                live.session.record().last_event_at,
            ),
            None => (
                record.last_state,
                DisplayState::from(record.last_state),
                record.last_event_at,
            ),
        };
        Self {
            session_id: record.session.as_str().to_owned(),
            repository_id: record.repository.map(|id| id.as_uuid()),
            note: record.note.clone(),
            tickets,
            cwd: record.cwd.clone(),
            cwd_exists: marked.cwd_exists,
            title,
            label,
            marked_at: record.marked_at.as_millis(),
            state: state.into(),
            display_state: display_state.into(),
            live: live.is_some(),
            last_event_at: last_event_at.as_millis(),
            resumed_as: record
                .resumed_as
                .iter()
                .map(|resumption| ResumptionDto {
                    session_id: resumption.session.as_str().to_owned(),
                    at: resumption.at.as_millis(),
                })
                .collect(),
            command,
            version: record.version.value(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/favorites",
    tag = "sessions",
    params(FavoritesQuery),
    responses((status = 200, body = FavoriteList), (status = 401, body = Problem))
)]
pub(crate) async fn list_favorites(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<FavoritesQuery>,
) -> Result<Json<FavoriteList>, ApiError> {
    let lines = state
        .favorites
        .list(query.repository_id.map(nullradix_domain::Id::from_uuid))
        .await?;
    Ok(Json(FavoriteList {
        items: lines.into_iter().map(FavoriteDto::from).collect(),
    }))
}

#[utoipa::path(
    put,
    path = "/sessions/{sessionId}/favorite",
    tag = "sessions",
    params(SessionPath),
    request_body = MarkFavorite,
    responses(
        (status = 200, body = FavoriteDto),
        (status = 401, body = Problem),
        (status = 404, description = "Neither a session nor a favorite", body = Problem),
        (status = 409, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn mark_favorite(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<SessionPath>,
    ApiJson(input): ApiJson<MarkFavorite>,
) -> Result<Json<FavoriteDto>, ApiError> {
    let line = state
        .favorites
        .mark(&path.session_id, input.note.as_deref())
        .await?;
    Ok(Json(line.into()))
}

#[utoipa::path(
    delete,
    path = "/sessions/{sessionId}/favorite",
    tag = "sessions",
    params(SessionPath),
    responses(
        (status = 204, description = "Not marked any more, also when it never was"),
        (status = 401, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn unmark_favorite(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<SessionPath>,
) -> Result<StatusCode, ApiError> {
    state.favorites.unmark(&path.session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{Value, json};

    use crate::tests::{TestApi, bearer, send, test_api};

    async fn call(api: &TestApi, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let request = bearer(Request::builder().method(method).uri(path), &api.token)
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    async fn start(api: &TestApi, session: &str, cwd: &str, source: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let event = json!({
            "id": uuid::Uuid::now_v7().to_string(),
            "sessionId": session,
            "cwd": cwd,
            "occurredAt": i64::try_from(now.as_millis()).unwrap(),
            "source": source,
        });
        let (status, reply) = call(api, "POST", "/api/v1/hooks/session-start", Some(event)).await;
        assert_eq!(status, 200, "{reply}");
    }

    #[tokio::test]
    async fn sessions_are_marked_listed_resumed_and_unmarked() {
        let api = test_api().await;
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        start(&api, "s-1", &cwd, "startup").await;
        let (status, marked) = call(
            &api,
            "PUT",
            "/api/v1/sessions/s-1/favorite",
            Some(json!({ "note": "login fix" })),
        )
        .await;
        assert_eq!(status, 200, "{marked}");
        assert_eq!(marked["note"], "login fix");
        assert_eq!(marked["live"], true);
        assert_eq!(marked["cwdExists"], true);
        assert_eq!(marked["displayState"], "idle");
        assert!(
            marked["command"]
                .as_str()
                .unwrap()
                .ends_with("&& claude --resume s-1")
        );
        let (status, kept) = call(
            &api,
            "PUT",
            "/api/v1/sessions/s-1/favorite",
            Some(json!({})),
        )
        .await;
        assert_eq!((status, &kept["note"]), (200, &json!("login fix")));

        start(&api, "s-2", &cwd, "resume").await;
        let (status, list) = call(&api, "GET", "/api/v1/favorites", None).await;
        assert_eq!(status, 200, "{list}");
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
        assert_eq!(list["items"][0]["resumedAs"][0]["sessionId"], "s-2");
        let (_, other) = call(
            &api,
            "GET",
            &format!("/api/v1/favorites?repositoryId={}", uuid::Uuid::nil()),
            None,
        )
        .await;
        assert_eq!(other["items"], json!([]));

        for _ in 0..2 {
            let (status, body) = call(&api, "DELETE", "/api/v1/sessions/s-1/favorite", None).await;
            assert_eq!((status, body), (204, Value::Null));
        }
        let (_, list) = call(&api, "GET", "/api/v1/favorites", None).await;
        assert_eq!(list["items"], json!([]));
    }

    #[tokio::test]
    async fn unknown_sessions_cannot_be_marked() {
        let api = test_api().await;
        let (status, problem) = call(
            &api,
            "PUT",
            "/api/v1/sessions/nope/favorite",
            Some(json!({})),
        )
        .await;
        assert_eq!(
            (status, &problem["code"]),
            (404, &json!("session-not-found"))
        );
        let (status, _) = call(
            &api,
            "PUT",
            "/api/v1/sessions/nope/favorite",
            Some(json!({ "note": 7 })),
        )
        .await;
        assert_eq!(status, 422);
    }
}
