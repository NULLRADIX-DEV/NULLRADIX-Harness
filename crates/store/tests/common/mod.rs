//! Doubles shared by the integration tests.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_app::repositories::{
    Located, RunnerService, not_a_git_project, path_missing, path_relative,
};
use nullradix_app::review::{
    BranchChanges, BranchFile, FilePatch, ReviewRequest, review_not_available,
};
use nullradix_app::runs::{RunProgress, RunSpec, RunnerPort};
use nullradix_app::session_detail::{
    ChangeRequest, DiffRequest, FileDiff, SessionChanges, TranscriptChunk, TranscriptRead,
    TranscriptSource, TranscriptSummary, change_not_found,
};
use nullradix_app::worktrees::{
    CreatedWorktree, EnsureWorktree, EnsuredWorktree, NewWorktree, RemoveWorktree, WorktreeRemoval,
};
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

    async fn create_worktree(&self, _request: &NewWorktree) -> Result<CreatedWorktree, AppError> {
        Err(AppError::unavailable("runner-failed", "runs nothing"))
    }

    async fn folders_exist(&self, paths: &[String]) -> Result<Vec<bool>, AppError> {
        Ok(vec![false; paths.len()])
    }

    async fn ensure_worktree(
        &self,
        _request: &EnsureWorktree,
    ) -> Result<EnsuredWorktree, AppError> {
        Err(AppError::unavailable("runner-failed", "runs nothing"))
    }

    async fn remove_worktree(
        &self,
        _request: &RemoveWorktree,
    ) -> Result<WorktreeRemoval, AppError> {
        Err(AppError::unavailable("runner-failed", "runs nothing"))
    }
    async fn read_transcript(
        &self,
        _request: &TranscriptRead,
    ) -> Result<Option<TranscriptChunk>, AppError> {
        Ok(None)
    }

    async fn transcript_facts(
        &self,
        _source: &TranscriptSource,
    ) -> Result<Option<TranscriptSummary>, AppError> {
        Ok(None)
    }

    async fn session_changes(&self, _request: &ChangeRequest) -> Result<SessionChanges, AppError> {
        Ok(SessionChanges::none())
    }

    async fn session_diff(&self, request: &DiffRequest) -> Result<FileDiff, AppError> {
        Err(change_not_found(&request.path))
    }

    async fn review_changes(&self, _request: &ReviewRequest) -> Result<BranchChanges, AppError> {
        Err(review_not_available("this runner reads no branches"))
    }

    async fn review_patch(
        &self,
        _request: &ReviewRequest,
        _path: &str,
    ) -> Result<FilePatch, AppError> {
        Err(review_not_available("this runner reads no branches"))
    }

    async fn review_file(
        &self,
        _root: &str,
        _branch: &str,
        _path: &str,
    ) -> Result<BranchFile, AppError> {
        Err(review_not_available("this runner reads no branches"))
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
