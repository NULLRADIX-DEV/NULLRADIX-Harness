use axum::Json;
use axum::extract::State;
use nullradix_app::repositories::RepositorySummary;
use nullradix_domain::{Runner, RunnerOs};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{ApiError, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RunnerOsDto {
    Windows,
    Linux,
}

impl From<RunnerOs> for RunnerOsDto {
    fn from(os: RunnerOs) -> Self {
        match os {
            RunnerOs::Windows => Self::Windows,
            RunnerOs::Linux => Self::Linux,
        }
    }
}

/// A machine that checks out repositories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunnerDto {
    pub id: Uuid,
    pub name: String,
    pub os: RunnerOsDto,
    /// Whether this is the daemon's own machine.
    pub local: bool,
}

impl RunnerDto {
    fn new(runner: &Runner, local: &Runner) -> Self {
        Self {
            id: runner.id().as_uuid(),
            name: runner.name().as_str().to_owned(),
            os: runner.os().into(),
            local: runner.id() == local.id(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunnerList {
    pub items: Vec<RunnerDto>,
}

/// Where a repository is checked out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutDto {
    pub runner_id: Uuid,
    /// Root of the main checkout with forward slashes.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryDto {
    pub id: Uuid,
    pub name: String,
    pub checkouts: Vec<CheckoutDto>,
}

impl From<RepositorySummary> for RepositoryDto {
    fn from(summary: RepositorySummary) -> Self {
        Self {
            id: summary.repository.id().as_uuid(),
            name: summary.repository.name().as_str().to_owned(),
            checkouts: summary
                .checkouts
                .iter()
                .map(|checkout| CheckoutDto {
                    runner_id: checkout.runner().as_uuid(),
                    path: checkout.path().as_str().to_owned(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RepositoryList {
    pub items: Vec<RepositoryDto>,
}

#[utoipa::path(
    get,
    path = "/runners",
    tag = "repositories",
    responses(
        (status = 200, body = RunnerList),
        (status = 401, body = Problem),
    )
)]
pub(crate) async fn list_runners(
    State(state): State<ApiState>,
) -> Result<Json<RunnerList>, ApiError> {
    let local = state.runners.local();
    let items = state
        .runners
        .list()
        .await?
        .iter()
        .map(|runner| RunnerDto::new(runner, local))
        .collect();
    Ok(Json(RunnerList { items }))
}

#[utoipa::path(
    get,
    path = "/repositories",
    tag = "repositories",
    responses(
        (status = 200, body = RepositoryList),
        (status = 401, body = Problem),
    )
)]
pub(crate) async fn list_repositories(
    State(state): State<ApiState>,
) -> Result<Json<RepositoryList>, ApiError> {
    let items = state
        .runners
        .repositories()
        .await?
        .into_iter()
        .map(RepositoryDto::from)
        .collect();
    Ok(Json(RepositoryList { items }))
}
