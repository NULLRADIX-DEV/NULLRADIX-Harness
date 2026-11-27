use axum::extract::State;
use axum::{Extension, Json};
use nullradix_app::AppError;
use nullradix_app::auth::Principal;
use nullradix_app::worktrees::{
    Cleaned, DoneRequest, Finished, Resumed, StartMode, Started, TargetCreated, WorktreeRemoval,
};
use nullradix_domain::Id;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::AgentContext;
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
    /// Name to write under, such as `claude`; the acting person when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Agent session of the caller; a name then writes as a session of that name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
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

/// What a resume did with the ticket's worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ResumeOutcome {
    /// The worktree is on its branch, added again when its folder was gone.
    Ready,
    /// The worktree was cleaned up; the ticket's branch and worktree are cleared.
    Reset,
    /// The branch is not in the repository on the runner.
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResumedTicket {
    pub ticket: TicketDto,
    pub outcome: ResumeOutcome,
    /// Folder of the worktree on the runner, with forward slashes; only when ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// The folder was added again.
    pub restored: bool,
    /// Where a leftover folder in the way was moved to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_aside: Option<String>,
    /// Why the worktree is not used, when the outcome is not ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
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
        (status = 403, description = "`decision-needs-person`: the request says it comes from a run or session of an agent and the ticket waits for a person", body = Problem),
        (status = 404, description = "Unknown ticket, host ticket or runner", body = Problem),
        (status = 409, description = "The ticket changed meanwhile", body = Problem),
        (status = 422, description = "Refused by a start rule, a lock or git", body = Problem),
        (status = 503, description = "The runner is not connected", body = Problem),
    )
)]
pub(crate) async fn start_ticket(
    State(state): State<ApiState>,
    Extension(context): Extension<AgentContext>,
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
            context.is_agent(),
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
        (status = 403, description = "`decision-needs-person`: the request says it comes from a run or session of an agent and the ticket waits for a person", body = Problem),
        (status = 404, description = "Unknown ticket or runner", body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "Refused by a lock or the board's repository", body = Problem),
        (status = 503, description = "The runner is not connected", body = Problem),
    )
)]
pub(crate) async fn finish_ticket(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Extension(context): Extension<AgentContext>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<FinishTicket>,
) -> Result<Json<FinishedTicket>, ApiError> {
    let actor = crate::tickets::author_of(
        &state,
        &principal,
        context,
        input.author.as_deref(),
        input.session.as_deref(),
        &path.ticket_key,
    )
    .await?;
    let request = DoneRequest {
        acceptance: input.acceptance.as_deref(),
        expected: input.version.map(AppError::client_version).transpose()?,
        options: writer(&principal, context, input.force),
    };
    let finished = state
        .worktrees
        .done(
            &path.ticket_key,
            Id::from_uuid(input.runner_id),
            &input.cwd,
            request,
            &actor,
        )
        .await?;
    Ok(Json(finished.into()))
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/resume",
    tag = "tickets",
    params(TicketPath),
    responses(
        (status = 200, body = ResumedTicket),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown ticket or runner", body = Problem),
        (status = 409, description = "The ticket changed meanwhile", body = Problem),
        (status = 422, description = "Not started, or the folder belongs to something else", body = Problem),
        (status = 503, description = "The runner is not connected", body = Problem),
    )
)]
pub(crate) async fn resume_ticket(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
) -> Result<Json<ResumedTicket>, ApiError> {
    let resumed = state
        .worktrees
        .resume(&path.ticket_key, &state.person)
        .await?;
    let ticket = state.tickets.get(&path.ticket_key).await?.view.into();
    let (outcome, ready, message) = match resumed {
        Resumed::Ready(ready) => (ResumeOutcome::Ready, Some(ready), None),
        Resumed::Reset(text) => (ResumeOutcome::Reset, None, Some(text)),
        Resumed::Skipped(text) => (ResumeOutcome::Skipped, None, Some(text)),
    };
    Ok(Json(ResumedTicket {
        ticket,
        outcome,
        restored: ready.as_ref().is_some_and(|ready| ready.restored),
        moved_aside: ready.as_ref().and_then(|ready| ready.moved_aside.clone()),
        worktree: ready.map(|ready| ready.path),
        message,
    }))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::{Command, Stdio};

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};

    use crate::tests::{TestApi, bearer, in_run, in_session, send, test_api};

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

    /// A repository with a bound board and the ticket ARC-1; returns its folder and runner.
    async fn bound_repository(api: &TestApi) -> (tempfile::TempDir, String, uuid::Uuid) {
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

    #[tokio::test]
    async fn starts_a_ticket_in_a_new_worktree_and_refuses_both_join_options() {
        let api = test_api().await;
        let (_dir, cwd, runner) = bound_repository(&api).await;
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

    #[tokio::test]
    async fn a_run_neither_starts_nor_finishes_a_ticket_that_waits_for_a_person() {
        let api = test_api().await;
        let (dir, cwd, runner) = bound_repository(&api).await;
        let hold = json!({
            "status": "blocked", "blockedReason": "Human Interaction: look", "version": 1
        });
        let request = bearer(Request::patch("/api/v1/tickets/arc-1"), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(hold.to_string()))
            .unwrap();
        assert_eq!(send(&api.router, request).await.0, StatusCode::OK);
        let plain = json!({ "runnerId": runner, "cwd": cwd });
        let forced = json!({ "runnerId": runner, "cwd": cwd, "force": true });
        let finish = json!({ "runnerId": runner, "cwd": cwd, "acceptance": "It works" });
        for mark in [in_run, in_session] {
            for (uri, body) in [
                ("/api/v1/tickets/arc-1/start", &plain),
                ("/api/v1/tickets/arc-1/start", &forced),
                ("/api/v1/tickets/arc-1/done", &finish),
            ] {
                let request = mark(bearer(Request::post(uri), &api.token))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap();
                let (status, _, problem) = send(&api.router, request).await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{uri} {problem}");
                assert_eq!(problem["code"], "decision-needs-person", "{uri}");
            }
        }
        // git made neither a folder nor a branch before the refusal
        assert!(!dir.path().join("arches-worktrees").exists());
        let branches = Command::new("git")
            .args(["branch", "--list", "ticket/*"])
            .current_dir(&cwd)
            .output()
            .unwrap();
        assert!(branches.stdout.is_empty(), "{branches:?}");
        // the person at a terminal starts it
        let (status, started) = post(&api, "/api/v1/tickets/arc-1/start", &plain).await;
        assert_eq!(status, StatusCode::OK, "{started}");
    }

    #[tokio::test]
    async fn done_inside_a_session_is_written_by_the_named_agent() {
        let api = test_api().await;
        let (_dir, cwd, runner) = bound_repository(&api).await;
        let body = json!({ "runnerId": runner, "cwd": cwd });
        post(&api, "/api/v1/tickets/arc-1/start", &body).await;
        let finish = json!({
            "runnerId": runner, "cwd": cwd, "acceptance": "It works",
            "author": "worker-ARC-0001", "session": "s-1"
        });
        let (status, finished) = post(&api, "/api/v1/tickets/arc-1/done", &finish).await;
        assert_eq!(status, StatusCode::OK, "{finished}");
        let request = bearer(Request::get("/api/v1/tickets/arc-1"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (_, _, detail) = send(&api.router, request).await;
        let done = detail["history"]
            .as_array()
            .unwrap()
            .iter()
            .find(|line| line["field"] == "status" && line["to"] == "done")
            .unwrap();
        assert_eq!(done["actorName"], "worker-ARC-0001");
    }

    #[tokio::test]
    async fn resumes_a_started_ticket_whose_folder_is_gone() {
        let api = test_api().await;
        let (_dir, cwd, runner) = bound_repository(&api).await;
        let resume = "/api/v1/tickets/arc-1/resume";
        let none = json!({});
        let (status, problem) = post(&api, resume, &none).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some("ticket-not-started"))
        );
        let (status, problem) = post(&api, "/api/v1/tickets/arc-9/resume", &none).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (StatusCode::NOT_FOUND, Some("ticket-not-found"))
        );
        let body = json!({ "runnerId": runner, "cwd": cwd });
        let (_, started) = post(&api, "/api/v1/tickets/arc-1/start", &body).await;
        let worktree = started["worktree"].as_str().unwrap().to_owned();
        let (status, same) = post(&api, resume, &none).await;
        assert_eq!(status, StatusCode::OK, "{same}");
        assert_eq!(same["outcome"], "ready");
        assert_eq!(same["restored"], false);
        assert_eq!(same["worktree"], worktree.as_str());
        std::fs::remove_dir_all(&worktree).unwrap();
        let (status, back) = post(&api, resume, &none).await;
        assert_eq!(status, StatusCode::OK, "{back}");
        assert_eq!(back["outcome"], "ready");
        assert_eq!(back["restored"], true);
        assert_eq!(back["ticket"]["branch"], started["branch"]);
        assert!(Path::new(&worktree).join(".git").exists());
        let finish = json!({ "runnerId": runner, "cwd": cwd, "acceptance": "It works" });
        let (status, _) = post(&api, "/api/v1/tickets/arc-1/done", &finish).await;
        assert_eq!(status, StatusCode::OK);
        let (status, reset) = post(&api, resume, &none).await;
        assert_eq!(status, StatusCode::OK, "{reset}");
        assert_eq!(reset["outcome"], "reset");
        assert!(reset["message"].as_str().unwrap().contains("cleaned up"));
        assert!(reset.get("worktree").is_none());
        assert!(reset["ticket"].get("branch").is_none_or(Value::is_null));
    }
}
