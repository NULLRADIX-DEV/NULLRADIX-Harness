//! Doubles shared by the integration tests.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_app::repositories::{
    Located, RunnerService, not_a_git_project, path_missing, path_relative,
};
use nullradix_app::runs::{RunProgress, RunSpec, RunnerPort};
use nullradix_app::{AppError, ChangeSignal, SystemClock, UuidV7Generator};
use nullradix_domain::{Id, Run};
use nullradix_store::Store;

/// Runner double that runs nothing; folders below `/work/<name>` are checkouts of `<name>`.
pub(crate) struct FolderRunner;

#[async_trait]
impl RunnerPort for FolderRunner {
    async fn check(&self, _spec: &RunSpec) -> Result<(), AppError> {
        Err(AppError::unavailable("runner-failed", "runs nothing"))
    }

    async fn start(&self, _run: Id<Run>, _spec: &RunSpec) -> Result<u32, AppError> {
        Err(AppError::unavailable("runner-failed", "runs nothing"))
    }

    async fn cancel(&self, _run: Id<Run>) -> Result<(), AppError> {
        Ok(())
    }

    async fn progress(
        &self,
        _run: Id<Run>,
        _after: i64,
        _limit: u32,
    ) -> Result<RunProgress, AppError> {
        Err(AppError::unavailable("runner-failed", "runs nothing"))
    }

    async fn locate(&self, path: &str) -> Result<Located, AppError> {
        let path = path.trim();
        if path.is_empty() {
            return Err(path_missing());
        }
        if !path.starts_with('/') {
            return Err(path_relative(path));
        }
        let mut parts = path.trim_start_matches('/').split('/');
        match (parts.next(), parts.next()) {
            (Some("work"), Some(name)) if !name.is_empty() => Ok(Located {
                root: format!("/work/{name}"),
                key: format!("/work/{}", name.to_lowercase()),
                name: name.to_owned(),
            }),
            _ => Err(not_a_git_project(path)),
        }
    }
}

pub(crate) async fn runners(store: &Arc<Store>, signal: &ChangeSignal) -> Arc<RunnerService> {
    Arc::new(
        RunnerService::start(
            store.clone(),
            store.clone(),
            Arc::new(FolderRunner),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        )
        .await
        .unwrap(),
    )
}
