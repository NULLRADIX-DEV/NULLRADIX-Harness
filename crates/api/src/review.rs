use axum::Json;
use axum::extract::{Extension, State};
use nullradix_app::AppError;
use nullradix_app::auth::{Principal, decision_needs_person};
use nullradix_app::review::{BranchFile, ChangedFile, FilePatch, ReviewView};
use nullradix_domain::{ReviewState, TestRun, Version};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::auth::{AgentContext, refuse_agent};
use crate::tickets::{TicketDto, TicketPath};
use crate::{ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

/// A file the ticket's branch changed against its target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFileDto {
    pub path: String,
    /// The earlier path of a renamed or copied file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// The first letter of git's status: A, M, D, R, C or T.
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
}

impl From<ChangedFile> for ReviewFileDto {
    fn from(file: ChangedFile) -> Self {
        Self {
            path: file.path,
            old_path: file.old_path,
            status: file.status,
            additions: file.additions,
            deletions: file.deletions,
            binary: file.binary,
        }
    }
}

/// A command the agent ran to check its work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestRunDto {
    pub command: String,
    /// Unknown when the call has no result.
    pub ok: Option<bool>,
    /// The end of its output.
    pub output: String,
}

impl From<TestRun> for TestRunDto {
    fn from(test: TestRun) -> Self {
        Self {
            command: test.command,
            ok: test.ok,
            output: test.output,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewStateDto {
    Pending,
    Approved,
    Returned,
}

impl From<ReviewState> for ReviewStateDto {
    fn from(state: ReviewState) -> Self {
        match state {
            ReviewState::Pending => Self::Pending,
            ReviewState::Approved => Self::Approved,
            ReviewState::Returned => Self::Returned,
        }
    }
}

/// What a person needs to review a ticket's branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDto {
    pub ticket_key: String,
    pub branch: Option<String>,
    /// The target branch, or the branch of the main checkout when the target does not exist.
    pub target: Option<String>,
    /// The merge base commit.
    pub base: Option<String>,
    pub files: Vec<ReviewFileDto>,
    /// More files changed than are listed.
    pub files_truncated: bool,
    /// The tests the agent ran in the run that finished the ticket.
    pub tests: Vec<TestRunDto>,
    /// When the review was opened, in Unix milliseconds.
    pub tests_at: Option<i64>,
    pub run_id: Option<Uuid>,
    /// Absent until a finished run opened a review.
    pub state: Option<ReviewStateDto>,
    /// False when the changes cannot be shown; `reason` says why.
    pub available: bool,
    pub reason: Option<String>,
}

impl From<ReviewView> for ReviewDto {
    fn from(view: ReviewView) -> Self {
        Self {
            ticket_key: view.ticket,
            branch: view.branch,
            target: view.target,
            base: view.base,
            files: view.files.into_iter().map(Into::into).collect(),
            files_truncated: view.files_truncated,
            tests: view.tests.into_iter().map(Into::into).collect(),
            tests_at: view.tests_at.map(nullradix_domain::Timestamp::as_millis),
            run_id: view.run.map(|run| run.as_uuid()),
            state: view.state.map(Into::into),
            available: view.available,
            reason: view.reason,
        }
    }
}

/// The patch of one changed file in git's unified format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPatchDto {
    pub path: String,
    pub patch: String,
    /// The patch is longer than the limit and was cut.
    pub truncated: bool,
}

/// A file as it is at the head of the ticket's branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFileContentDto {
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    pub binary: bool,
    /// The content; absent for a binary file.
    pub text: Option<String>,
    /// The file is larger than the limit and `text` holds its start.
    pub truncated: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct DiffQuery {
    /// Path of a changed file, as the review lists it.
    path: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct ReviewFilePath {
    /// Ticket key such as `ARC-0007`; case and padding do not matter.
    ticket_key: String,
    /// Path of the file inside the repository.
    path: String,
}

fn patch_dto(file: String, found: FilePatch) -> ReviewPatchDto {
    ReviewPatchDto {
        path: file,
        patch: found.patch,
        truncated: found.truncated,
    }
}

fn content_dto(name: String, found: BranchFile) -> ReviewFileContentDto {
    ReviewFileContentDto {
        path: name,
        size: found.size,
        binary: found.binary,
        text: found.text,
        truncated: found.truncated,
    }
}

#[utoipa::path(
    get,
    path = "/tickets/{ticketKey}/review",
    tag = "review",
    params(TicketPath),
    responses(
        (status = 200, body = ReviewDto),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown ticket", body = Problem),
        (status = 422, description = "A malformed ticket key", body = Problem),
        (status = 503, description = "The runner is not connected", body = Problem),
    )
)]
pub(crate) async fn get_review(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
) -> Result<Json<ReviewDto>, ApiError> {
    Ok(Json(state.reviews.view(&path.ticket_key).await?.into()))
}

#[utoipa::path(
    get,
    path = "/tickets/{ticketKey}/review/diff",
    tag = "review",
    params(TicketPath, DiffQuery),
    responses(
        (status = 200, body = ReviewPatchDto),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown ticket", body = Problem),
        (status = 422, description = "No branch to review, a file outside the changes or git failed", body = Problem),
        (status = 503, description = "The runner is not connected", body = Problem),
    )
)]
pub(crate) async fn get_review_diff(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiQuery(query): ApiQuery<DiffQuery>,
) -> Result<Json<ReviewPatchDto>, ApiError> {
    let found = state.reviews.patch(&path.ticket_key, &query.path).await?;
    Ok(Json(patch_dto(query.path, found)))
}

#[utoipa::path(
    get,
    path = "/tickets/{ticketKey}/review/files/{*path}",
    tag = "review",
    params(ReviewFilePath),
    responses(
        (status = 200, body = ReviewFileContentDto),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown ticket", body = Problem),
        (status = 422, description = "No branch to review, or a path that is no file of the branch", body = Problem),
        (status = 503, description = "The runner is not connected", body = Problem),
    )
)]
pub(crate) async fn get_review_file(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ReviewFilePath>,
) -> Result<Json<ReviewFileContentDto>, ApiError> {
    let file = state.reviews.file(&path.ticket_key, &path.path).await?;
    Ok(Json(content_dto(path.path, file)))
}

/// Accepts a ticket held for review; the version guards against a stale view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptReview {
    pub version: Option<i64>,
}

/// Sends a ticket held for review back to the agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReturnReview {
    /// What the agent changes next, at most 10000 characters.
    pub feedback: String,
    pub version: Option<i64>,
}

fn version(value: Option<i64>) -> Result<Option<Version>, AppError> {
    value.map(AppError::client_version).transpose()
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/review/accept",
    tag = "review",
    params(TicketPath),
    request_body = AcceptReview,
    responses(
        (status = 200, description = "Accepted, also when it was before", body = TicketDto),
        (status = 401, body = Problem),
        (status = 403, description = "`decision-needs-person`: the request says it comes from a run or session of an agent", body = Problem),
        (status = 404, description = "Unknown ticket", body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "The ticket waits for no review", body = Problem),
    )
)]
pub(crate) async fn accept_review(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Extension(context): Extension<AgentContext>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<AcceptReview>,
) -> Result<Json<TicketDto>, ApiError> {
    refuse_agent(context, || decision_needs_person("accepting a review"))?;
    let view = state
        .decisions
        .accept(
            &path.ticket_key,
            &state.person,
            version(input.version)?,
            matches!(principal, Principal::Session { .. }),
        )
        .await?;
    Ok(Json(view.into()))
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/review/return",
    tag = "review",
    params(TicketPath),
    request_body = ReturnReview,
    responses(
        (status = 200, description = "Back to planned, also when it was before", body = TicketDto),
        (status = 401, body = Problem),
        (status = 403, description = "`decision-needs-person`: the request says it comes from a run or session of an agent", body = Problem),
        (status = 404, description = "Unknown ticket", body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "No feedback, too long, or the ticket waits for no review", body = Problem),
    )
)]
pub(crate) async fn return_review(
    State(state): State<ApiState>,
    Extension(context): Extension<AgentContext>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<ReturnReview>,
) -> Result<Json<TicketDto>, ApiError> {
    refuse_agent(context, || decision_needs_person("returning a review"))?;
    let view = state
        .decisions
        .return_to_agent(
            &path.ticket_key,
            &input.feedback,
            &state.person,
            version(input.version)?,
        )
        .await?;
    Ok(Json(view.into()))
}
#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::{Command, Stdio};

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};

    use crate::tests::{
        TestApi, as_person, bearer, in_run, in_session, local, open_session, send, test_api,
    };

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    async fn get(api: &TestApi, path: &str) -> (StatusCode, Value) {
        let request = bearer(Request::get(path), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status, json)
    }

    async fn post(api: &TestApi, path: &str, body: &Value) -> (StatusCode, Value) {
        let request = bearer(Request::post(path), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status, json)
    }

    /// A repository with a bound board and the ticket ARC-1; returns its folder and runner.
    async fn bound_repository(api: &TestApi) -> (tempfile::TempDir, String, uuid::Uuid) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("arches");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);
        std::fs::write(root.join("keep.txt"), "keep\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "init"]);
        let cwd = root.to_string_lossy().into_owned();
        let project = api.projects.create("ARC", "Arches").await.unwrap();
        let board = api
            .boards
            .create(project.id(), "Main", &api.person)
            .await
            .unwrap()
            .board;
        let runner = api.runners.local().id();
        api.boards
            .set_repository(project.id(), board.id(), runner, &cwd, board.version())
            .await
            .unwrap();
        let tickets = format!(
            "/api/v1/projects/{}/boards/{}/tickets",
            project.id().as_uuid(),
            board.id().as_uuid()
        );
        let (status, _) = post(api, &tickets, &json!({ "title": "Fix it" })).await;
        assert_eq!(status, StatusCode::CREATED);
        (dir, cwd, runner.as_uuid())
    }

    async fn patch(api: &TestApi, path: &str, body: &Value) -> (StatusCode, Value) {
        let request = bearer(Request::patch(path), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status, json)
    }

    #[tokio::test]
    async fn a_ticket_that_waits_for_no_review_can_be_neither_accepted_nor_returned() {
        let api = test_api().await;
        let (_dir, _cwd, _runner) = bound_repository(&api).await;
        let (status, problem) = post(&api, "/api/v1/tickets/arc-1/review/accept", &json!({})).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Some("accept-not-possible")
            )
        );
        let body = json!({ "feedback": "more tests" });
        let (status, problem) = post(&api, "/api/v1/tickets/arc-1/review/return", &body).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Some("return-not-possible")
            )
        );
        let (status, problem) = post(&api, "/api/v1/tickets/arc-9/review/accept", &json!({})).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (StatusCode::NOT_FOUND, Some("ticket-not-found"))
        );
        let (status, _) = post(&api, "/api/v1/tickets/arc-1/review/return", &json!({})).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        for path in ["accept", "return"] {
            let request = local(Request::post(format!(
                "/api/v1/tickets/arc-1/review/{path}"
            )))
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
            let (status, _, _) = send(&api.router, request).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        }
    }

    /// The ticket ARC-1 started on a branch and blocked for a person; returns its version.
    async fn held(api: &TestApi) -> (tempfile::TempDir, i64) {
        let (dir, cwd, runner) = bound_repository(api).await;
        let body = json!({ "runnerId": runner, "cwd": cwd });
        let (status, started) = post(api, "/api/v1/tickets/arc-1/start", &body).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let version = started["ticket"]["version"].as_i64().unwrap();
        let blocked = json!({
            "version": version,
            "status": "blocked",
            "blockedReason": "Human Interaction: review before merge",
            "acceptance": "It works",
        });
        let (status, ticket) = patch(api, "/api/v1/tickets/arc-1", &blocked).await;
        assert_eq!(status, StatusCode::OK, "{ticket}");
        (dir, ticket["ticket"]["version"].as_i64().unwrap())
    }

    #[tokio::test]
    async fn returning_a_held_ticket_sends_it_back_to_planned_with_a_marker_comment() {
        let api = test_api().await;
        let (_dir, version) = held(&api).await;
        let path = "/api/v1/tickets/arc-1/review/return";
        let (status, problem) = post(&api, path, &json!({ "feedback": "  " })).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some("feedback-empty"))
        );
        let long = "x".repeat(10_001);
        let (status, problem) = post(&api, path, &json!({ "feedback": long })).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some("value-too-long"))
        );
        let stale = json!({ "feedback": "x", "version": version - 1 });
        let (status, problem) = post(&api, path, &stale).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (StatusCode::CONFLICT, Some("version-conflict"))
        );
        let body = json!({ "feedback": "Add a test", "version": version });
        let (status, ticket) = post(&api, path, &body).await;
        assert_eq!(status, StatusCode::OK, "{ticket}");
        assert_eq!(ticket["status"], "planned");
        assert_eq!(ticket["blockedReason"], Value::Null);
        let (_, comments) = get(&api, "/api/v1/tickets/arc-1/comments").await;
        let last = comments["items"].as_array().unwrap().last().unwrap();
        assert_eq!(last["systemMarker"], "review-returned", "{comments}");
        assert!(
            last["body"]
                .as_str()
                .is_some_and(|text| text.starts_with("[dispatch] review-returned (by ")
                    && text.ends_with("): Add a test")),
            "{last}"
        );
    }

    #[tokio::test]
    async fn accepting_a_held_ticket_finishes_it() {
        let api = test_api().await;
        let (_dir, version) = held(&api).await;
        let stale = json!({ "version": version - 1 });
        let (status, _) = post(&api, "/api/v1/tickets/arc-1/review/accept", &stale).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let body = json!({ "version": version });
        let (status, ticket) = post(&api, "/api/v1/tickets/arc-1/review/accept", &body).await;
        assert_eq!(status, StatusCode::OK, "{ticket}");
        assert_eq!(ticket["status"], "done");
    }

    /// Every way to accept `path` that names a run or a session is refused; the person may.
    async fn accepting_stays_with_the_person(path: &str) {
        let api = test_api().await;
        let (_dir, version) = held(&api).await;
        let cookie = open_session(&api).await;
        let body = json!({ "version": version }).to_string();
        for mark in [in_run, in_session] {
            // a token and a browser session alike are refused when they name an agent
            for as_session in [false, true] {
                let builder = if as_session {
                    as_person(Request::post(path), &cookie)
                } else {
                    bearer(Request::post(path), &api.token)
                };
                let request = mark(builder)
                    .header("content-type", "application/json")
                    .body(Body::from(body.clone()))
                    .unwrap();
                let (status, _, problem) = send(&api.router, request).await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{path} {problem}");
                assert_eq!(problem["code"], "decision-needs-person", "{path}");
            }
        }
        let (_, held) = get(&api, "/api/v1/tickets/arc-1").await;
        assert_eq!(held["ticket"]["status"], "blocked");

        let request = as_person(Request::post(path), &cookie)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let (status, _, ticket) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::OK, "{ticket}");
        assert_eq!(ticket["status"], "done");
    }

    #[tokio::test]
    async fn a_review_is_returned_by_the_person_and_not_by_a_run_or_session_of_an_agent() {
        let api = test_api().await;
        let (_dir, version) = held(&api).await;
        let cookie = open_session(&api).await;
        let path = "/api/v1/tickets/arc-1/review/return";
        let body = json!({ "feedback": "Add a test", "version": version }).to_string();
        for mark in [in_run, in_session] {
            for as_session in [false, true] {
                let builder = if as_session {
                    as_person(Request::post(path), &cookie)
                } else {
                    bearer(Request::post(path), &api.token)
                };
                let request = mark(builder)
                    .header("content-type", "application/json")
                    .body(Body::from(body.clone()))
                    .unwrap();
                let (status, _, problem) = send(&api.router, request).await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
                assert_eq!(problem["code"], "decision-needs-person");
            }
        }
        let (_, held) = get(&api, "/api/v1/tickets/arc-1").await;
        assert_eq!(held["ticket"]["status"], "blocked");
        let request = as_person(Request::post(path), &cookie)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let (status, _, ticket) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::OK, "{ticket}");
        assert_eq!(ticket["status"], "planned");
    }

    #[tokio::test]
    async fn a_review_is_accepted_by_the_person_and_not_by_a_run_or_session_of_an_agent() {
        accepting_stays_with_the_person("/api/v1/tickets/arc-1/review/accept").await;
    }

    #[tokio::test]
    async fn a_ticket_is_accepted_by_the_person_and_not_by_a_run_or_session_of_an_agent() {
        accepting_stays_with_the_person("/api/v1/tickets/arc-1/accept").await;
    }

    #[tokio::test]
    async fn a_ticket_without_a_branch_has_nothing_to_review() {
        let api = test_api().await;
        let (_dir, _cwd, _runner) = bound_repository(&api).await;
        let (status, review) = get(&api, "/api/v1/tickets/arc-1/review").await;
        assert_eq!(status, StatusCode::OK, "{review}");
        assert_eq!(review["ticketKey"], "ARC-0001");
        assert_eq!(review["available"], false);
        assert_eq!(review["reason"], "the ticket has no branch");
        assert_eq!(review["files"], json!([]));
        assert_eq!(review["tests"], json!([]));
        for path in [
            "/api/v1/tickets/arc-1/review/diff?path=a.txt",
            "/api/v1/tickets/arc-1/review/files/a.txt",
        ] {
            let (status, problem) = get(&api, path).await;
            assert_eq!(
                (status, problem["code"].as_str()),
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Some("review-not-available")
                ),
                "{path}"
            );
        }
        let (status, problem) = get(&api, "/api/v1/tickets/arc-9/review").await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (StatusCode::NOT_FOUND, Some("ticket-not-found"))
        );
        let request = local(Request::get("/api/v1/tickets/arc-1/review"))
            .body(Body::empty())
            .unwrap();
        let (status, _, _) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn shows_the_changes_patches_and_files_of_a_started_ticket() {
        let api = test_api().await;
        let (_dir, cwd, runner) = bound_repository(&api).await;
        let body = json!({ "runnerId": runner, "cwd": cwd });
        let (status, started) = post(&api, "/api/v1/tickets/arc-1/start", &body).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let worktree = started["worktree"].as_str().unwrap().to_owned();
        let tree = Path::new(&worktree);
        std::fs::create_dir_all(tree.join("src")).unwrap();
        std::fs::write(tree.join("src/a.txt"), "hello\n").unwrap();
        std::fs::write(tree.join("keep.txt"), "kept\n").unwrap();
        std::fs::write(tree.join("logo.bin"), [0u8, 1, 2, 0]).unwrap();
        git(tree, &["add", "-A"]);
        git(tree, &["commit", "-q", "-m", "work"]);
        // the cleanup of a done ticket removes the folder; the branch stays readable
        let finish = json!({ "runnerId": runner, "cwd": cwd, "acceptance": "It works" });
        let (status, finished) = post(&api, "/api/v1/tickets/arc-1/done", &finish).await;
        assert_eq!(status, StatusCode::OK, "{finished}");
        assert!(!tree.exists());

        let (status, review) = get(&api, "/api/v1/tickets/arc-1/review").await;
        assert_eq!(status, StatusCode::OK, "{review}");
        assert_eq!(review["available"], true);
        assert_eq!(review["branch"], "ticket/ARC-0001-fix-it");
        assert!(review["base"].as_str().is_some_and(|base| base.len() == 40));
        assert!(review["target"].as_str().is_some());
        assert_eq!(review["state"], Value::Null);
        let files = review["files"].as_array().unwrap();
        let find = |path: &str| files.iter().find(|file| file["path"] == path).unwrap();
        assert_eq!(files.len(), 3, "{review}");
        assert_eq!(find("src/a.txt")["status"], "A");
        assert_eq!(find("src/a.txt")["additions"], 1);
        assert_eq!(find("keep.txt")["status"], "M");
        assert_eq!(find("logo.bin")["binary"], true);

        let (status, diff) = get(&api, "/api/v1/tickets/arc-1/review/diff?path=keep.txt").await;
        assert_eq!(status, StatusCode::OK, "{diff}");
        assert_eq!(diff["path"], "keep.txt");
        assert_eq!(diff["truncated"], false);
        let patch = diff["patch"].as_str().unwrap();
        assert!(
            patch.contains("-keep") && patch.contains("+kept"),
            "{patch}"
        );

        let (status, file) = get(&api, "/api/v1/tickets/arc-1/review/files/src/a.txt").await;
        assert_eq!(status, StatusCode::OK, "{file}");
        assert_eq!(file["path"], "src/a.txt");
        assert_eq!(file["text"], "hello\n");
        assert_eq!(file["size"], 6);
        assert_eq!(file["binary"], false);
        let (_, binary) = get(&api, "/api/v1/tickets/arc-1/review/files/logo.bin").await;
        assert_eq!(binary["binary"], json!(true));
        assert_eq!(binary["text"], Value::Null);
    }

    #[tokio::test]
    async fn refuses_paths_outside_the_changes_and_outside_the_repository() {
        let api = test_api().await;
        let (_dir, cwd, runner) = bound_repository(&api).await;
        let body = json!({ "runnerId": runner, "cwd": cwd });
        let (_, started) = post(&api, "/api/v1/tickets/arc-1/start", &body).await;
        let tree = started["worktree"].as_str().unwrap().to_owned();
        std::fs::create_dir_all(Path::new(&tree).join("src")).unwrap();
        std::fs::write(Path::new(&tree).join("src/a.txt"), "hello\n").unwrap();
        git(Path::new(&tree), &["add", "-A"]);
        git(Path::new(&tree), &["commit", "-q", "-m", "work"]);
        let unknown = [
            "/api/v1/tickets/arc-1/review/diff?path=keep.txt",
            "/api/v1/tickets/arc-1/review/diff?path=..%2Fx",
            "/api/v1/tickets/arc-1/review/diff?path=",
            "/api/v1/tickets/arc-1/review/files/src",
            "/api/v1/tickets/arc-1/review/files/..%2F..%2Fx",
            "/api/v1/tickets/arc-1/review/files/missing.txt",
        ];
        for path in unknown {
            let (status, problem) = get(&api, path).await;
            assert_eq!(
                (status, problem["code"].as_str()),
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Some("review-file-unknown")
                ),
                "{path}"
            );
        }
        let (status, _) = get(&api, "/api/v1/tickets/arc-1/review/diff").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}
