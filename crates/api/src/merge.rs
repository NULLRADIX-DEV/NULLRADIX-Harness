use axum::Json;
use axum::extract::State;
use nullradix_app::AppError;
use nullradix_app::dispatch::merge_jobs::StoredMergeJob;
use nullradix_app::dispatch::merge_retry::RetryOutcome;
use nullradix_domain::MergePhase;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::tickets::{TicketDto, TicketPath};
use crate::{ApiError, ApiJson, ApiPath, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MergePhaseDto {
    Pending,
    Waiting,
    ConflictSession,
    Merged,
    Failed,
    Cancelled,
}

impl From<MergePhase> for MergePhaseDto {
    fn from(phase: MergePhase) -> Self {
        match phase {
            MergePhase::Pending => Self::Pending,
            MergePhase::Waiting => Self::Waiting,
            MergePhase::ConflictSession => Self::ConflictSession,
            MergePhase::Merged => Self::Merged,
            MergePhase::Failed => Self::Failed,
            MergePhase::Cancelled => Self::Cancelled,
        }
    }
}

/// The merge of a worktree's branch into its target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MergeJobDto {
    pub id: Uuid,
    pub branch: String,
    pub phase: MergePhaseDto,
    /// None: the branch the main checkout has checked out.
    pub target_branch: Option<String>,
    pub strategy: crate::MergeStrategyDto,
    /// A merge conflict was found and undone.
    pub conflict_seen: bool,
    /// Why the merge failed, or the files it conflicts in.
    pub problem: Option<String>,
    pub merged_into: Option<String>,
    pub merged_commit: Option<String>,
    /// Unix milliseconds the merge was queued.
    pub queued_at: i64,
    pub finished_at: Option<i64>,
    pub version: i64,
}

impl From<StoredMergeJob> for MergeJobDto {
    fn from(stored: StoredMergeJob) -> Self {
        let record = stored.job.record();
        Self {
            id: record.id.as_uuid(),
            branch: stored.branch,
            phase: record.phase.into(),
            target_branch: record.target_branch.clone(),
            strategy: record.strategy.into(),
            conflict_seen: record.conflict_seen,
            problem: record.problem.clone(),
            merged_into: record.merged_into.clone(),
            merged_commit: record.merged_commit.clone(),
            queued_at: record.since.as_millis(),
            finished_at: record
                .finished_at
                .map(nullradix_domain::Timestamp::as_millis),
            version: record.version.value(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MergeJobList {
    pub items: Vec<MergeJobDto>,
}

#[utoipa::path(
    get,
    path = "/tickets/{ticketKey}/merges",
    tag = "merge",
    params(TicketPath),
    responses(
        (status = 200, body = MergeJobList),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown ticket", body = Problem),
        (status = 422, description = "A malformed ticket key", body = Problem),
    )
)]
pub(crate) async fn list_merges(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
) -> Result<Json<MergeJobList>, ApiError> {
    let detail = state.tickets.get(&path.ticket_key).await?;
    let jobs = state.merges.of_ticket(detail.view.ticket.id()).await?;
    Ok(Json(MergeJobList {
        items: jobs.into_iter().map(Into::into).collect(),
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MergeOutcomeDto {
    Merged,
    Blocked,
    Waiting,
    Session,
    Pending,
}

impl From<RetryOutcome> for MergeOutcomeDto {
    fn from(outcome: RetryOutcome) -> Self {
        match outcome {
            RetryOutcome::Merged => Self::Merged,
            RetryOutcome::Blocked => Self::Blocked,
            RetryOutcome::Waiting => Self::Waiting,
            RetryOutcome::Session => Self::Session,
            RetryOutcome::Pending => Self::Pending,
        }
    }
}

/// Tries the merge of a done or merge-blocked ticket again; the version guards a stale view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryMerge {
    pub version: Option<i64>,
}

/// What the retried merge came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MergeRetryDto {
    pub outcome: MergeOutcomeDto,
    /// One line for the person, such as `ARC-0001 merged into main (abc1234).`
    pub text: String,
    pub ticket: TicketDto,
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/merge",
    tag = "merge",
    params(TicketPath),
    request_body = RetryMerge,
    responses(
        (status = 200, description = "Queued again and tried; a repeat changes nothing", body = MergeRetryDto),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown ticket", body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "The ticket waits for no merge", body = Problem),
    )
)]
pub(crate) async fn retry_merge(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<RetryMerge>,
) -> Result<Json<MergeRetryDto>, ApiError> {
    let version = input.version.map(AppError::client_version).transpose()?;
    let retried = state
        .retries
        .retry(&path.ticket_key, &state.person, version)
        .await?;
    Ok(Json(MergeRetryDto {
        outcome: retried.outcome.into(),
        text: retried.text,
        ticket: retried.view.into(),
    }))
}
