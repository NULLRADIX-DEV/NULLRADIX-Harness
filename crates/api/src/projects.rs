use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use nullradix_app::AppError;
use nullradix_domain::{Id, Project};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{API_PREFIX, ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

/// A project as the API shows it; timestamps are Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDto {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: i64,
}

impl From<Project> for ProjectDto {
    fn from(project: Project) -> Self {
        Self {
            id: project.id().as_uuid(),
            key: project.key().to_string(),
            name: project.name().as_str().to_owned(),
            created_at: project.created_at().as_millis(),
            updated_at: project.updated_at().as_millis(),
            version: project.version().value(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProjectList {
    pub items: Vec<ProjectDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateProject {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameProject {
    pub name: String,
    /// Version the change is based on; a newer stored version fails with `version-conflict`.
    pub version: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct DeleteProjectQuery {
    /// Version the deletion is based on.
    version: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct ProjectPath {
    /// Project id.
    project_id: Uuid,
}

#[utoipa::path(
    get,
    path = "/projects",
    tag = "projects",
    responses((status = 200, body = ProjectList), (status = 401, body = Problem))
)]
pub(crate) async fn list_projects(
    State(state): State<ApiState>,
) -> Result<Json<ProjectList>, ApiError> {
    let items = state
        .projects
        .list()
        .await?
        .into_iter()
        .map(ProjectDto::from)
        .collect();
    Ok(Json(ProjectList { items }))
}

#[utoipa::path(
    post,
    path = "/projects",
    tag = "projects",
    request_body = CreateProject,
    responses(
        (status = 201, body = ProjectDto,
            headers(("Location" = String, description = "URL of the new project"))),
        (status = 401, body = Problem),
        (status = 409, description = "The key is taken", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn create_project(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<CreateProject>,
) -> Result<(StatusCode, HeaderMap, Json<ProjectDto>), ApiError> {
    let project = ProjectDto::from(state.projects.create(&input.key, &input.name).await?);
    let mut headers = HeaderMap::new();
    let location = format!("{API_PREFIX}/projects/{}", project.id);
    if let Ok(value) = HeaderValue::from_str(&location) {
        headers.insert(header::LOCATION, value);
    }
    Ok((StatusCode::CREATED, headers, Json(project)))
}

#[utoipa::path(
    get,
    path = "/projects/{projectId}",
    tag = "projects",
    params(ProjectPath),
    responses(
        (status = 200, body = ProjectDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn get_project(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ProjectPath>,
) -> Result<Json<ProjectDto>, ApiError> {
    let project = state.projects.get(Id::from_uuid(path.project_id)).await?;
    Ok(Json(project.into()))
}

#[utoipa::path(
    patch,
    path = "/projects/{projectId}",
    tag = "projects",
    params(ProjectPath),
    request_body = RenameProject,
    responses(
        (status = 200, body = ProjectDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn rename_project(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ProjectPath>,
    ApiJson(input): ApiJson<RenameProject>,
) -> Result<Json<ProjectDto>, ApiError> {
    let project = state
        .projects
        .rename(
            Id::from_uuid(path.project_id),
            &input.name,
            AppError::client_version(input.version)?,
        )
        .await?;
    Ok(Json(project.into()))
}

#[utoipa::path(
    delete,
    path = "/projects/{projectId}",
    tag = "projects",
    params(ProjectPath, DeleteProjectQuery),
    responses(
        (status = 204, description = "The project is gone"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn delete_project(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ProjectPath>,
    ApiQuery(query): ApiQuery<DeleteProjectQuery>,
) -> Result<StatusCode, ApiError> {
    state
        .projects
        .delete(
            Id::from_uuid(path.project_id),
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

    use crate::tests::{TestApi, bearer, send, test_api};

    async fn call(
        api: &TestApi,
        request: axum::http::request::Builder,
        body: Option<serde_json::Value>,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        let request = bearer(request, &api.token);
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status, json)
    }

    #[tokio::test]
    async fn creates_reads_renames_and_deletes_projects() {
        let api = test_api().await;
        let (status, created) = call(
            &api,
            Request::post("/api/v1/projects"),
            Some(json!({ "key": "ARC", "name": "Arches" })),
        )
        .await;
        assert_eq!(status, 201);
        assert_eq!(created["key"], "ARC");
        assert_eq!(created["version"], 1);
        let id = created["id"].as_str().unwrap().to_owned();

        let (status, list) = call(&api, Request::get("/api/v1/projects"), None).await;
        assert_eq!(status, 200);
        assert_eq!(list["items"][0]["id"], id.as_str());

        let (status, renamed) = call(
            &api,
            Request::patch(format!("/api/v1/projects/{id}")),
            Some(json!({ "name": "Arches 2", "version": 1 })),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(renamed["name"], "Arches 2");
        assert_eq!(renamed["version"], 2);

        let (status, stale) = call(
            &api,
            Request::delete(format!("/api/v1/projects/{id}?version=1")),
            None,
        )
        .await;
        assert_eq!(
            (status.as_u16(), stale["code"].as_str()),
            (409, Some("version-conflict"))
        );

        let (status, _) = call(
            &api,
            Request::delete(format!("/api/v1/projects/{id}?version=2")),
            None,
        )
        .await;
        assert_eq!(status, 204);
        let (status, gone) = call(&api, Request::get(format!("/api/v1/projects/{id}")), None).await;
        assert_eq!(
            (status.as_u16(), gone["code"].as_str()),
            (404, Some("project-not-found"))
        );
    }

    #[tokio::test]
    async fn created_projects_point_to_their_url() {
        use tower::ServiceExt;
        let api = test_api().await;
        let request = bearer(Request::post("/api/v1/projects"), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "key": "ARC", "name": "Arches" }).to_string(),
            ))
            .unwrap();
        let response = api.router.clone().oneshot(request).await.unwrap();
        let location = response.headers()["location"].to_str().unwrap().to_owned();
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let project: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            location,
            format!("/api/v1/projects/{}", project["id"].as_str().unwrap())
        );
    }

    #[tokio::test]
    async fn stale_renames_conflict() {
        let api = test_api().await;
        let (_, created) = call(
            &api,
            Request::post("/api/v1/projects"),
            Some(json!({ "key": "ARC", "name": "Arches" })),
        )
        .await;
        let id = created["id"].as_str().unwrap().to_owned();
        call(
            &api,
            Request::patch(format!("/api/v1/projects/{id}")),
            Some(json!({ "name": "Two", "version": 1 })),
        )
        .await;
        let (status, problem) = call(
            &api,
            Request::patch(format!("/api/v1/projects/{id}")),
            Some(json!({ "name": "Three", "version": 1 })),
        )
        .await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (409, Some("version-conflict"))
        );
        let (status, problem) = call(
            &api,
            Request::patch(format!("/api/v1/projects/{id}")),
            Some(json!({ "name": "Three", "version": 0 })),
        )
        .await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (422, Some("invalid-version"))
        );
    }

    #[tokio::test]
    async fn validates_input_with_problem_details() {
        let api = test_api().await;
        let (status, problem) = call(
            &api,
            Request::post("/api/v1/projects"),
            Some(json!({ "key": "arc", "name": "Arches" })),
        )
        .await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (422, Some("invalid-project-key"))
        );

        let (status, problem) = call(
            &api,
            Request::post("/api/v1/projects"),
            Some(json!({ "key": "ARC", "name": "Arches", "extra": true })),
        )
        .await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (422, Some("invalid-body"))
        );

        let (status, problem) = call(&api, Request::get("/api/v1/projects/not-a-uuid"), None).await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (422, Some("invalid-path"))
        );

        call(
            &api,
            Request::post("/api/v1/projects"),
            Some(json!({ "key": "ARC", "name": "Arches" })),
        )
        .await;
        let (status, problem) = call(
            &api,
            Request::post("/api/v1/projects"),
            Some(json!({ "key": "ARC", "name": "Again" })),
        )
        .await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (409, Some("project-key-taken"))
        );
    }
}
