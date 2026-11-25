use axum::Json;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use nullradix_app::auth::Principal;
use nullradix_app::notes::{NewNote, NoteRow, PromotedNote as Promoted, Promotion, Writer};
use nullradix_domain::{Id, NoteState};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::tickets::{PriorityDto, TicketDto, TicketStatusDto};
use crate::{API_PREFIX, ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum NoteStateDto {
    Open,
    /// Turned into a ticket.
    Promoted,
    Dropped,
}

impl From<NoteState> for NoteStateDto {
    fn from(state: NoteState) -> Self {
        match state {
            NoteState::Open => Self::Open,
            NoteState::Promoted => Self::Promoted,
            NoteState::Dropped => Self::Dropped,
        }
    }
}

/// A finding kept for a later ticket; timestamps are Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NoteDto {
    /// Number people type, such as 12.
    pub number: i64,
    pub id: Uuid,
    pub board_id: Uuid,
    pub author_id: Uuid,
    pub author_name: String,
    pub title: String,
    pub body: String,
    /// Key of the ticket it was found at.
    pub source: Option<String>,
    pub labels: Vec<String>,
    /// How often it stood out, the first time included.
    pub seen_count: u32,
    /// Other tickets it stood out at again.
    pub seen_in: Vec<String>,
    pub state: NoteStateDto,
    /// Key of the ticket it was promoted to.
    pub ticket_key: Option<String>,
    /// Why it was dropped.
    pub reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: i64,
}

impl From<NoteRow> for NoteDto {
    fn from(row: NoteRow) -> Self {
        let note = row.note;
        Self {
            seen_count: u32::try_from(note.seen_count()).unwrap_or(u32::MAX),
            seen_in: note.seen_in().iter().map(ToString::to_string).collect(),
            number: note.number,
            id: note.id.as_uuid(),
            board_id: note.board.as_uuid(),
            author_id: note.author.as_uuid(),
            author_name: row.author_name,
            source: note.source.map(|source| source.key.to_string()),
            labels: note.labels,
            state: note.state.into(),
            ticket_key: note.promoted_to.map(|ticket| ticket.key.to_string()),
            reason: note.reason,
            created_at: note.created_at.as_millis(),
            updated_at: note.updated_at.as_millis(),
            version: note.version.value(),
            title: note.title,
            body: note.body,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NoteList {
    pub items: Vec<NoteDto>,
}

/// A new note: a title that makes sense without context and optional details.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateNote {
    pub title: String,
    /// Where, how to see it, suspected cause, proposal.
    pub body: Option<String>,
    /// Key of the ticket it was found at; it must belong to the board's project.
    pub source: Option<String>,
    /// Entries may hold several labels split by commas.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Name to note it under, such as `claude`; the acting person when left out.
    pub author: Option<String>,
    /// Adds it even when a similar open or promoted note exists.
    #[serde(default)]
    pub force: bool,
}

/// The finding stood out again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeenNote {
    /// Key of the ticket it stood out at.
    pub source: Option<String>,
}

/// What the ticket made from a note gets instead of the note's values.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromoteNote {
    /// The note's title when left out.
    pub title: Option<String>,
    /// Backlog when left out; read as text, so an unknown one is `status-invalid`.
    #[schema(value_type = Option<TicketStatusDto>)]
    pub status: Option<String>,
    /// Normal when left out.
    #[schema(value_type = Option<PriorityDto>)]
    pub priority: Option<String>,
    /// Replaces the note's labels.
    pub labels: Option<Vec<String>>,
    /// Name to create the ticket under, such as `claude`; the acting person when left out.
    pub by: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DropNote {
    /// Why nothing comes of it; required.
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PromotedNote {
    pub note: NoteDto,
    pub ticket: TicketDto,
}

impl From<Promoted> for PromotedNote {
    fn from(promoted: Promoted) -> Self {
        Self {
            note: promoted.note.into(),
            ticket: promoted.ticket.into(),
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct BoardNotesPath {
    /// Project id.
    project_id: Uuid,
    /// Board id.
    board_id: Uuid,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct NotePath {
    /// Note number such as `12`.
    note_number: String,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct NoteQuery {
    /// `open`, `promoted`, `dropped` or `all`; only open notes when left out.
    state: Option<String>,
    /// Only notes found at this ticket key.
    source: Option<String>,
}

fn writer<'a>(principal: &Principal, name: Option<&'a str>) -> Writer<'a> {
    Writer {
        by_person: matches!(principal, Principal::Session { .. }),
        name,
    }
}

#[utoipa::path(
    get,
    path = "/projects/{projectId}/boards/{boardId}/notes",
    tag = "notes",
    params(BoardNotesPath, NoteQuery),
    responses(
        (status = 200, description = "Notes by number", body = NoteList),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn list_notes(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<BoardNotesPath>,
    ApiQuery(query): ApiQuery<NoteQuery>,
) -> Result<Json<NoteList>, ApiError> {
    let items = state
        .notes
        .list(
            Id::from_uuid(path.project_id),
            Id::from_uuid(path.board_id),
            query.state.as_deref(),
            query.source.as_deref(),
        )
        .await?
        .into_iter()
        .map(NoteDto::from)
        .collect();
    Ok(Json(NoteList { items }))
}

#[utoipa::path(
    post,
    path = "/projects/{projectId}/boards/{boardId}/notes",
    tag = "notes",
    params(BoardNotesPath),
    request_body = CreateNote,
    responses(
        (status = 201, body = NoteDto,
            headers(("Location" = String, description = "URL of the new note"))),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "A similar note exists; `similar` names it", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn add_note(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<BoardNotesPath>,
    ApiJson(input): ApiJson<CreateNote>,
) -> Result<(StatusCode, HeaderMap, Json<NoteDto>), ApiError> {
    let note = NewNote {
        title: input.title,
        body: input.body,
        source: input.source,
        labels: input.labels,
        force: input.force,
    };
    let created = NoteDto::from(
        state
            .notes
            .add(
                Id::from_uuid(path.project_id),
                Id::from_uuid(path.board_id),
                &note,
                writer(&principal, input.author.as_deref()),
                &state.person,
            )
            .await?,
    );
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&format!("{API_PREFIX}/notes/{}", created.number)) {
        headers.insert(header::LOCATION, value);
    }
    Ok((StatusCode::CREATED, headers, Json(created)))
}

#[utoipa::path(
    get,
    path = "/notes/{noteNumber}",
    tag = "notes",
    params(NotePath),
    responses(
        (status = 200, body = NoteDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn get_note(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<NotePath>,
) -> Result<Json<NoteDto>, ApiError> {
    Ok(Json(state.notes.get(&path.note_number).await?.into()))
}

#[utoipa::path(
    post,
    path = "/notes/{noteNumber}/seen",
    tag = "notes",
    params(NotePath),
    request_body = SeenNote,
    responses(
        (status = 200, body = NoteDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "The note is promoted or dropped", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn seen_note(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<NotePath>,
    ApiJson(input): ApiJson<SeenNote>,
) -> Result<Json<NoteDto>, ApiError> {
    let note = state
        .notes
        .seen(&path.note_number, input.source.as_deref())
        .await?;
    Ok(Json(note.into()))
}

#[utoipa::path(
    post,
    path = "/notes/{noteNumber}/promote",
    tag = "notes",
    params(NotePath),
    request_body = PromoteNote,
    responses(
        (status = 200, description = "The note and the ticket made from it", body = PromotedNote),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "The note is not open", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn promote_note(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<NotePath>,
    ApiJson(input): ApiJson<PromoteNote>,
) -> Result<Json<PromotedNote>, ApiError> {
    let promotion = Promotion {
        title: input.title,
        status: input.status,
        priority: input.priority,
        labels: input.labels,
    };
    let promoted = state
        .notes
        .promote(
            &path.note_number,
            &promotion,
            writer(&principal, input.by.as_deref()),
            &state.person,
        )
        .await?;
    Ok(Json(promoted.into()))
}

#[utoipa::path(
    post,
    path = "/notes/{noteNumber}/drop",
    tag = "notes",
    params(NotePath),
    request_body = DropNote,
    responses(
        (status = 200, body = NoteDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "The note is promoted", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn drop_note(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<NotePath>,
    ApiJson(input): ApiJson<DropNote>,
) -> Result<Json<NoteDto>, ApiError> {
    let note = state
        .notes
        .drop_note(&path.note_number, &input.reason)
        .await?;
    Ok(Json(note.into()))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{Value, json};

    use crate::tests::{TestApi, bearer, local, open_session, send, test_api};

    async fn notes_path(api: &TestApi) -> String {
        let project = api.projects.create("ARC", "Arches").await.unwrap();
        let board = api
            .boards
            .create(project.id(), "Main", &api.person)
            .await
            .unwrap()
            .board;
        format!(
            "/api/v1/projects/{}/boards/{}/notes",
            project.id().as_uuid(),
            board.id().as_uuid()
        )
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
    async fn notes_are_added_listed_seen_and_refused_when_similar() {
        let api = test_api().await;
        let path = notes_path(&api).await;
        let body = json!({
            "title": "Dispatcher starts the worktree twice",
            "body": "WorktreeManager creates the branch twice",
            "labels": ["git, dispatch"],
            "author": "claude",
        });
        let request = bearer(Request::post(path.as_str()), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = tower::ServiceExt::oneshot(api.router.clone(), request)
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        assert_eq!(response.headers()["location"], "/api/v1/notes/1");

        let similar = json!({ "title": "WorktreeManager: branch twice, dispatcher" });
        let (status, problem) = post(&api, &path, &similar).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (409, Some("note-similar"))
        );
        assert_eq!(
            problem["similar"],
            json!([{
                "number": 1,
                "title": "Dispatcher starts the worktree twice",
                "state": "open",
                "ticketKey": null,
            }])
        );

        let (status, note) = post(&api, "/api/v1/notes/1/seen", &json!({})).await;
        assert_eq!((status, note["seenCount"].as_u64()), (200, Some(2)));
        assert_eq!(note["authorName"], "claude");
        assert_eq!(note["labels"], json!(["git", "dispatch"]));

        let (status, list) = get(&api, &format!("{path}?state=all")).await;
        assert_eq!(status, 200);
        assert_eq!(list["items"].as_array().map(Vec::len), Some(1));
        let (status, problem) = get(&api, &format!("{path}?state=taken")).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("state-invalid"))
        );
        let (status, problem) = get(&api, "/api/v1/notes/abc").await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("note-id-invalid"))
        );
        let (status, problem) = get(&api, "/api/v1/notes/9").await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (404, Some("note-not-found"))
        );
    }

    #[tokio::test]
    async fn promoting_makes_a_ticket_and_dropping_needs_a_reason() {
        let api = test_api().await;
        let path = notes_path(&api).await;
        post(&api, &path, &json!({ "title": "Cache never expires" })).await;
        post(&api, &path, &json!({ "title": "Login is slow" })).await;

        let promote = json!({ "priority": "high", "by": "claude" });
        let (status, promoted) = post(&api, "/api/v1/notes/1/promote", &promote).await;
        assert_eq!(status, 200);
        assert_eq!(promoted["note"]["state"], "promoted");
        assert_eq!(promoted["note"]["ticketKey"], "ARC-0001");
        assert_eq!(promoted["ticket"]["title"], "Cache never expires");
        assert_eq!(promoted["ticket"]["priority"], "high");
        assert_eq!(promoted["ticket"]["createdByName"], "claude");
        let (status, problem) = post(&api, "/api/v1/notes/1/promote", &json!({})).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (409, Some("note-not-open"))
        );
        let (status, problem) = post(&api, "/api/v1/notes/1/drop", &json!({ "reason": "x" })).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (409, Some("note-promoted"))
        );

        let (status, problem) = post(&api, "/api/v1/notes/2/drop", &json!({})).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("reason-missing"))
        );
        let (status, note) = post(&api, "/api/v1/notes/2/drop", &json!({ "reason": "fine" })).await;
        assert_eq!((status, note["state"].as_str()), (200, Some("dropped")));
        assert_eq!(note["reason"], "fine");
    }

    #[tokio::test]
    async fn a_browser_session_writes_as_the_person() {
        let api = test_api().await;
        let path = notes_path(&api).await;
        let cookie = open_session(&api).await;
        let body = json!({ "title": "Typed in the board", "author": "ben" });
        let request = local(Request::post(path.as_str()))
            .header(axum::http::header::COOKIE, &cookie)
            .header(axum::http::header::ORIGIN, "http://127.0.0.1:4711")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, note) = send(&api.router, request).await;
        assert_eq!(status, 201);
        assert_eq!(note["authorName"], "ben");
    }
}
