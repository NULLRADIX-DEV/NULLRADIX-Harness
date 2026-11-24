use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use nullradix_app::AppError;
use nullradix_app::boards::BoardSummary;
use nullradix_domain::Id;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{API_PREFIX, ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

/// A board as the API shows it; timestamps are Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BoardDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub owner_id: Uuid,
    pub owner_name: String,
    /// Whether the dispatcher may take tickets from this board.
    pub dispatch_enabled: bool,
    /// Tickets that are neither done nor not needed.
    pub open_tickets: u32,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: i64,
}

impl From<BoardSummary> for BoardDto {
    fn from(summary: BoardSummary) -> Self {
        let board = summary.board;
        Self {
            id: board.id().as_uuid(),
            project_id: board.project().as_uuid(),
            name: board.name().as_str().to_owned(),
            owner_id: board.owner().as_uuid(),
            owner_name: summary.owner_name,
            dispatch_enabled: board.dispatch_enabled(),
            open_tickets: summary.open_tickets,
            created_at: board.created_at().as_millis(),
            updated_at: board.updated_at().as_millis(),
            version: board.version().value(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BoardList {
    pub items: Vec<BoardDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateBoard {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameBoard {
    pub name: String,
    /// Version the rename is based on.
    pub version: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct ProjectBoardsPath {
    /// Project id.
    project_id: Uuid,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct BoardPath {
    /// Project id.
    project_id: Uuid,
    /// Board id.
    board_id: Uuid,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct DeleteBoardQuery {
    /// Version the deletion is based on.
    version: i64,
}

#[utoipa::path(
    get,
    path = "/projects/{projectId}/boards",
    tag = "boards",
    params(ProjectBoardsPath),
    responses(
        (status = 200, body = BoardList),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn list_boards(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ProjectBoardsPath>,
) -> Result<Json<BoardList>, ApiError> {
    let project = Id::from_uuid(path.project_id);
    state.projects.get(project).await?;
    let items = state
        .boards
        .list(Some(project))
        .await?
        .into_iter()
        .map(BoardDto::from)
        .collect();
    Ok(Json(BoardList { items }))
}

#[utoipa::path(
    post,
    path = "/projects/{projectId}/boards",
    tag = "boards",
    params(ProjectBoardsPath),
    request_body = CreateBoard,
    responses(
        (status = 201, body = BoardDto,
            headers(("Location" = String, description = "URL of the new board"))),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn create_board(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ProjectBoardsPath>,
    ApiJson(input): ApiJson<CreateBoard>,
) -> Result<(StatusCode, HeaderMap, Json<BoardDto>), ApiError> {
    let project = Id::from_uuid(path.project_id);
    let board = BoardDto::from(
        state
            .boards
            .create(project, &input.name, &state.person)
            .await?,
    );
    let mut headers = HeaderMap::new();
    let location = format!(
        "{API_PREFIX}/projects/{}/boards/{}",
        board.project_id, board.id
    );
    if let Ok(value) = HeaderValue::from_str(&location) {
        headers.insert(header::LOCATION, value);
    }
    Ok((StatusCode::CREATED, headers, Json(board)))
}

#[utoipa::path(
    get,
    path = "/projects/{projectId}/boards/{boardId}",
    tag = "boards",
    params(BoardPath),
    responses(
        (status = 200, body = BoardDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn get_board(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<BoardPath>,
) -> Result<Json<BoardDto>, ApiError> {
    let board = state
        .boards
        .get(Id::from_uuid(path.project_id), Id::from_uuid(path.board_id))
        .await?;
    Ok(Json(board.into()))
}

#[utoipa::path(
    patch,
    path = "/projects/{projectId}/boards/{boardId}",
    tag = "boards",
    params(BoardPath),
    request_body = RenameBoard,
    responses(
        (status = 200, body = BoardDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn rename_board(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<BoardPath>,
    ApiJson(input): ApiJson<RenameBoard>,
) -> Result<Json<BoardDto>, ApiError> {
    let board = state
        .boards
        .rename(
            Id::from_uuid(path.project_id),
            Id::from_uuid(path.board_id),
            &input.name,
            AppError::client_version(input.version)?,
        )
        .await?;
    Ok(Json(board.into()))
}

#[utoipa::path(
    delete,
    path = "/projects/{projectId}/boards/{boardId}",
    tag = "boards",
    params(BoardPath, DeleteBoardQuery),
    responses(
        (status = 204, description = "The board is gone"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn delete_board(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<BoardPath>,
    ApiQuery(query): ApiQuery<DeleteBoardQuery>,
) -> Result<StatusCode, ApiError> {
    state
        .boards
        .delete(
            Id::from_uuid(path.project_id),
            Id::from_uuid(path.board_id),
            AppError::client_version(query.version)?,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;

    use crate::tests::{bearer, send, test_api};

    #[tokio::test]
    async fn boards_live_inside_their_project() {
        let api = test_api().await;
        let project = api.projects.create("ARC", "Arches").await.unwrap();
        let base = format!("/api/v1/projects/{}/boards", project.id().as_uuid());

        let request = bearer(Request::post(base.as_str()), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(json!({ "name": "Main" }).to_string()))
            .unwrap();
        let (status, _, board) = send(&api.router, request).await;
        assert_eq!(status, 201);
        assert_eq!(board["ownerName"], "tester");
        assert_eq!(board["openTickets"], 0);
        let location = format!("{base}/{}", board["id"].as_str().unwrap());

        let request = bearer(Request::get(base.as_str()), &api.token)
            .body(Body::empty())
            .unwrap();
        let (_, _, list) = send(&api.router, request).await;
        assert_eq!(list["items"][0]["name"], "Main");

        let request = bearer(Request::patch(location.as_str()), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "name": "Next", "version": 1 }).to_string(),
            ))
            .unwrap();
        let (status, _, renamed) = send(&api.router, request).await;
        assert_eq!(
            (status.as_u16(), renamed["name"].as_str()),
            (200, Some("Next"))
        );

        let path = format!("{location}?version=1");
        let request = bearer(Request::delete(path.as_str()), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, problem) = send(&api.router, request).await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (409, Some("version-conflict"))
        );

        let other = format!("/api/v1/projects/{}/boards", uuid::Uuid::now_v7());
        let request = bearer(Request::get(other.as_str()), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, problem) = send(&api.router, request).await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (404, Some("project-not-found"))
        );
    }
}
