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
    pub project_key: String,
    pub name: String,
    pub owner_id: Uuid,
    pub owner_name: String,
    /// Repository the board's work happens in.
    pub repository_id: Option<Uuid>,
    pub repository_name: Option<String>,
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
            project_key: summary.project_key,
            name: board.name().as_str().to_owned(),
            owner_id: board.owner().as_uuid(),
            owner_name: summary.owner_name,
            repository_id: board.repository().map(|id| id.as_uuid()),
            repository_name: summary.repository_name,
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

/// Points a board to the repository checked out at a folder on a runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetBoardRepository {
    pub runner_id: Uuid,
    /// Absolute folder on the runner; any folder inside the checkout or a worktree of it.
    pub path: String,
    /// Version the change is based on.
    pub version: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct AllBoardsQuery {
    /// With `path`: only boards of the repository checked out there on this runner.
    runner_id: Option<Uuid>,
    /// With `runnerId`: a folder inside the checkout.
    path: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct VersionQuery {
    /// Version the change is based on.
    version: i64,
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
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct DeleteBoardQuery {
    /// Version the deletion is based on.
    version: i64,
    /// Also delete tickets that are neither done nor not needed.
    #[serde(default)]
    delete_open_tickets: bool,
}

/// What a board deletion took with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BoardDeleted {
    /// Keys of the deleted tickets by number.
    pub deleted_tickets: Vec<String>,
}

fn board_list(summaries: Vec<BoardSummary>) -> BoardList {
    BoardList {
        items: summaries.into_iter().map(BoardDto::from).collect(),
    }
}

#[utoipa::path(
    get,
    path = "/boards",
    tag = "boards",
    params(AllBoardsQuery),
    responses(
        (status = 200, description = "Boards of every project, or of one checkout", body = BoardList),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown runner", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn list_all_boards(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<AllBoardsQuery>,
) -> Result<Json<BoardList>, ApiError> {
    let boards = match (query.runner_id, query.path) {
        (None, None) => state.boards.list(None).await?,
        (Some(runner), Some(path)) => {
            state
                .boards
                .list_by_checkout(Id::from_uuid(runner), &path)
                .await?
        }
        _ => {
            return Err(AppError::invalid("query-invalid", "runnerId and path go together").into());
        }
    };
    Ok(Json(board_list(boards)))
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
    Ok(Json(board_list(state.boards.list(Some(project)).await?)))
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
        (status = 200, description = "The board is gone with its tickets", body = BoardDeleted),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "Open tickets without deleteOpenTickets", body = Problem),
    )
)]
pub(crate) async fn delete_board(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<BoardPath>,
    ApiQuery(query): ApiQuery<DeleteBoardQuery>,
) -> Result<Json<BoardDeleted>, ApiError> {
    let deleted = state
        .boards
        .delete(
            Id::from_uuid(path.project_id),
            Id::from_uuid(path.board_id),
            AppError::client_version(query.version)?,
            query.delete_open_tickets,
            &state.person,
        )
        .await?;
    Ok(Json(BoardDeleted {
        deleted_tickets: deleted.iter().map(ToString::to_string).collect(),
    }))
}

#[utoipa::path(
    put,
    path = "/projects/{projectId}/boards/{boardId}/repository",
    tag = "boards",
    params(BoardPath),
    request_body = SetBoardRepository,
    responses(
        (status = 200, body = BoardDto),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown project, board or runner", body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "The path is no folder inside a git repository", body = Problem),
        (status = 503, description = "The runner is not connected", body = Problem),
    )
)]
pub(crate) async fn set_board_repository(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<BoardPath>,
    ApiJson(input): ApiJson<SetBoardRepository>,
) -> Result<Json<BoardDto>, ApiError> {
    let board = state
        .boards
        .set_repository(
            Id::from_uuid(path.project_id),
            Id::from_uuid(path.board_id),
            Id::from_uuid(input.runner_id),
            &input.path,
            AppError::client_version(input.version)?,
        )
        .await?;
    Ok(Json(board.into()))
}

#[utoipa::path(
    delete,
    path = "/projects/{projectId}/boards/{boardId}/repository",
    tag = "boards",
    params(BoardPath, VersionQuery),
    responses(
        (status = 200, description = "The board without a repository", body = BoardDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn clear_board_repository(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<BoardPath>,
    ApiQuery(query): ApiQuery<VersionQuery>,
) -> Result<Json<BoardDto>, ApiError> {
    let board = state
        .boards
        .clear_repository(
            Id::from_uuid(path.project_id),
            Id::from_uuid(path.board_id),
            AppError::client_version(query.version)?,
        )
        .await?;
    Ok(Json(board.into()))
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
        let request = bearer(Request::get("/api/v1/boards"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (_, _, all) = send(&api.router, request).await;
        assert_eq!(all["items"], list["items"]);

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

    fn git_repo(dir: &std::path::Path) -> String {
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success());
        dir.join("src").to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn boards_point_to_repositories_on_runners() {
        let api = test_api().await;
        let project = api.projects.create("ARC", "Arches").await.unwrap();
        let board = api
            .boards
            .create(project.id(), "Main", &api.person)
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("Arches");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let folder = git_repo(&repo);
        let get = |path: String| {
            bearer(Request::get(path), &api.token)
                .body(Body::empty())
                .unwrap()
        };

        let (_, _, runners) = send(&api.router, get(String::from("/api/v1/runners"))).await;
        assert_eq!(runners["items"][0]["name"], "local");
        assert_eq!(runners["items"][0]["local"], true);
        let runner = runners["items"][0]["id"].as_str().unwrap().to_owned();

        let location = format!(
            "/api/v1/projects/{}/boards/{}/repository",
            project.id().as_uuid(),
            board.board.id().as_uuid()
        );
        let put = |body: serde_json::Value| {
            bearer(Request::put(location.as_str()), &api.token)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let outside = dir.path().to_string_lossy().into_owned();
        let (status, _, problem) = send(
            &api.router,
            put(json!({ "runnerId": runner, "path": outside, "version": 1 })),
        )
        .await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (422, Some("not-a-git-project"))
        );
        let (status, _, bound) = send(
            &api.router,
            put(json!({ "runnerId": runner, "path": folder, "version": 1 })),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(bound["repositoryName"], "Arches");

        let (_, _, repositories) =
            send(&api.router, get(String::from("/api/v1/repositories"))).await;
        assert_eq!(repositories["items"][0]["id"], bound["repositoryId"]);
        let path = repositories["items"][0]["checkouts"][0]["path"]
            .as_str()
            .unwrap();
        assert!(path.ends_with("/Arches"), "{path}");

        let query = format!(
            "/api/v1/boards?runnerId={runner}&path={}",
            path.replace(' ', "%20")
        );
        let (_, _, found) = send(&api.router, get(query)).await;
        assert_eq!(found["items"][0]["id"], bound["id"]);
        let (status, _, problem) = send(
            &api.router,
            get(format!("/api/v1/boards?runnerId={runner}")),
        )
        .await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (422, Some("query-invalid"))
        );

        let clear = format!("{location}?version=2");
        let request = bearer(Request::delete(clear.as_str()), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, cleared) = send(&api.router, request).await;
        assert_eq!(status, 200);
        assert_eq!(cleared["repositoryId"], serde_json::Value::Null);
        assert_eq!(cleared["version"], 3);
    }
}
