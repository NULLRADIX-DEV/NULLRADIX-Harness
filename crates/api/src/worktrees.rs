use axum::extract::State;
use axum::{Extension, Json};
use nullradix_app::AppError;
use nullradix_app::auth::Principal;
use nullradix_app::worktrees::{
    Cleaned, DoneRequest, Finished, StartMode, Started, TargetCreated, WorktreeRemoval,
};
use nullradix_domain::Id;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::tickets::{TicketDto, TicketPath, writer};
use crate::{ApiError, ApiJson, ApiPath, ApiState, Problem};

/// Starts a ticket from a folder on a runner; `joinTicket` and `join` exclude each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartTicket {
    pub runner_id: Uuid,
    /// Absolute folder in the board's repository; without a target the branch starts at its HEAD.
    pub cwd: String,
    /// Joins this ticket's worktree; a different target gives a warning and an own worktree.
    pub join_ticket: Option<String>,
    /// Joins the first open worktree with the same target, else creates one.
    #[serde(default)]
    pub join: bool,
    /// Lifts the dependency and question locks.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TargetCreatedDto {
    pub branch: String,
    /// Such as `origin/release`, or the branch or commit of the main checkout.
    pub from: String,
}

impl From<TargetCreated> for TargetCreatedDto {
    fn from(created: TargetCreated) -> Self {
        Self {
            branch: created.branch,
            from: created.from,
        }
    }
}

/// A ready worktree the cleanup removed or kept; its branch stays either way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CleanedWorktree {
    /// Folder on the runner, with forward slashes.
    pub path: String,
    pub branch: String,
    pub removed: bool,
    /// Why the folder stays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_because: Option<String>,
}

impl From<Cleaned> for CleanedWorktree {
    fn from(cleaned: Cleaned) -> Self {
        let kept_because = match cleaned.removal {
            WorktreeRemoval::Removed => None,
            WorktreeRemoval::Kept(reason) => Some(reason),
            // cleanup never reports a missing folder; kept for completeness
            WorktreeRemoval::Missing => Some(String::from("the folder does not exist")),
        };
        Self {
            path: cleaned.path,
            branch: cleaned.branch,
            removed: kept_because.is_none(),
            kept_because,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartedTicket {
    pub ticket: TicketDto,
    pub branch: String,
    /// Folder of the worktree on the runner, with forward slashes.
    pub worktree: String,
    /// The ticket joined an existing worktree.
    pub joined: bool,
    pub warnings: Vec<String>,
    /// A missing target branch the start created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_created: Option<TargetCreatedDto>,
    /// Ready worktrees removed or kept before the start.
    pub cleanup: Vec<CleanedWorktree>,
}

impl From<Started> for StartedTicket {
    fn from(started: Started) -> Self {
        Self {
            ticket: started.ticket.into(),
            branch: started.branch,
            worktree: started.worktree,
            joined: started.joined,
            warnings: started.warnings,
            target_created: started.target_created.map(Into::into),
            cleanup: started.cleanup.into_iter().map(Into::into).collect(),
        }
    }
}

/// Finishes a ticket from a folder on a runner, as legacy `board done`; nothing is merged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinishTicket {
    pub runner_id: Uuid,
    /// Absolute folder in the board's repository; a worktree containing it stays.
    pub cwd: String,
    /// Acceptance criteria set in the same step; done needs them here or on the ticket.
    pub acceptance: Option<String>,
    /// Version the change is based on; without one it checks none.
    pub version: Option<i64>,
    /// Lifts the dependency, question and not-needed locks, never the acceptance lock.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FinishedTicket {
    pub ticket: TicketDto,
    /// The ticket's branch, when it runs in a worktree.
    pub branch: Option<String>,
    /// Every ticket still on the branch is finished; nothing was merged.
    pub ready_to_merge: bool,
    pub cleanup: Vec<CleanedWorktree>,
}

impl From<Finished> for FinishedTicket {
    fn from(finished: Finished) -> Self {
        Self {
            ticket: finished.ticket.into(),
            branch: finished.branch,
            ready_to_merge: finished.ready_to_merge,
            cleanup: finished.cleanup.into_iter().map(Into::into).collect(),
        }
    }
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/start",
    tag = "tickets",
    params(TicketPath),
    request_body = StartTicket,
    responses(
        (status = 200, body = StartedTicket),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown ticket, host ticket or runner", body = Problem),
        (status = 409, description = "The ticket changed meanwhile", body = Problem),
        (status = 422, description = "Refused by a start rule, a lock or git", body = Problem),
        (status = 503, description = "The runner is not connected", body = Problem),
    )
)]
pub(crate) async fn start_ticket(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<StartTicket>,
) -> Result<Json<StartedTicket>, ApiError> {
    let mode = StartMode::of(input.join_ticket, input.join)?;
    let started = state
        .worktrees
        .start(
            &path.ticket_key,
            Id::from_uuid(input.runner_id),
            &input.cwd,
            mode,
            input.force,
            &state.person,
        )
        .await?;
    Ok(Json(started.into()))
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/done",
    tag = "tickets",
    params(TicketPath),
    request_body = FinishTicket,
    responses(
        (status = 200, body = FinishedTicket),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown ticket or runner", body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "Refused by a lock or the board's repository", body = Problem),
        (status = 503, description = "The runner is not connected", body = Problem),
    )
)]
pub(crate) async fn finish_ticket(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<FinishTicket>,
) -> Result<Json<FinishedTicket>, ApiError> {
    let request = DoneRequest {
        acceptance: input.acceptance.as_deref(),
        expected: input.version.map(AppError::client_version).transpose()?,
        options: writer(&principal, input.force),
    };
    let finished = state
        .worktrees
        .done(
            &path.ticket_key,
            Id::from_uuid(input.runner_id),
            &input.cwd,
            request,
            &state.person,
        )
        .await?;
    Ok(Json(finished.into()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::{Command, Stdio};

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};

    use crate::tests::{TestApi, bearer, send, test_api};

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    async fn post(api: &TestApi, path: &str, body: &Value) -> (StatusCode, Value) {
        let request = bearer(Request::post(path), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status, json)
    }

    #[tokio::test]
    async fn starts_a_ticket_in_a_new_worktree_and_refuses_both_join_options() {
        let api = test_api().await;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("arches");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let cwd = root.to_string_lossy().into_owned();
        let project = api.projects.create("ARC", "Arches").await.unwrap();
        let board = api
            .boards
            .create(project.id(), "Main", &api.person)
            .await
            .unwrap()
            .board;
        let runner = api.runners.local().id().as_uuid();
        api.boards
            .set_repository(
                project.id(),
                board.id(),
                api.runners.local().id(),
                &cwd,
                board.version(),
            )
            .await
            .unwrap();
        let tickets = format!(
            "/api/v1/projects/{}/boards/{}/tickets",
            project.id().as_uuid(),
            board.id().as_uuid()
        );
        let (status, _) = post(&api, &tickets, &json!({ "title": "Fix it" })).await;
        assert_eq!(status, StatusCode::CREATED);
        let start = "/api/v1/tickets/arc-1/start";
        let both = json!({ "runnerId": runner, "cwd": cwd, "joinTicket": "ARC-2", "join": true });
        let (status, problem) = post(&api, start, &both).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some("input-invalid"))
        );
        let body = json!({ "runnerId": runner, "cwd": cwd });
        let (status, started) = post(&api, start, &body).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        assert_eq!(started["branch"], "ticket/ARC-0001-fix-it");
        assert_eq!(started["joined"], false);
        assert_eq!(started["ticket"]["status"], "in-progress");
        assert!(started.get("targetCreated").is_none());
        let worktree = started["worktree"].as_str().unwrap();
        assert!(
            worktree.ends_with("/arches-worktrees/ARC-0001"),
            "{worktree}"
        );
        assert!(Path::new(worktree).is_dir());
        let (status, problem) = post(&api, start, &body).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some("already-started"))
        );
        let done = "/api/v1/tickets/arc-1/done";
        let (status, problem) = post(&api, done, &body).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some("acceptance-missing"))
        );
        let finish = json!({ "runnerId": runner, "cwd": cwd, "acceptance": "It works" });
        let (status, finished) = post(&api, done, &finish).await;
        assert_eq!(status, StatusCode::OK, "{finished}");
        assert_eq!(finished["ticket"]["status"], "done");
        assert_eq!(finished["branch"], "ticket/ARC-0001-fix-it");
        assert_eq!(finished["readyToMerge"], true);
        assert_eq!(finished["cleanup"][0]["path"], worktree);
        assert_eq!(finished["cleanup"][0]["removed"], true, "{finished}");
        assert!(!Path::new(worktree).exists());
    }
}
