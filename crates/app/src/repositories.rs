//! Runners check out repositories; boards point to the repository their work happens in.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Checkout, CheckoutPath, Id, Repository, RepositoryError, RepositoryName, Runner, RunnerName,
    RunnerOs,
};

use crate::review::{BranchChanges, BranchFile, FilePatch, ReviewRequest};
use crate::runs::RunnerPort;
use crate::session_detail::{
    ChangeRequest, DiffRequest, FileDiff, SessionChanges, TranscriptChunk, TranscriptRead,
    TranscriptSource, TranscriptSummary,
};
use crate::worktrees::{
    CreatedWorktree, EnsureWorktree, EnsuredWorktree, NewWorktree, RemoveWorktree, WorktreeRemoval,
};
use crate::{AppError, Change, ChangeSignal, Clock, ErrorKind, IdGenerator};

/// Name of the runner on the daemon's own machine.
pub const LOCAL_RUNNER: &str = "local";

/// The repository a folder on a runner belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    /// Root of the main checkout, also for a worktree, with forward slashes.
    pub root: String,
    /// Comparable spelling of `root`; equal keys mean the same folder.
    pub key: String,
    /// Last segment of `root`.
    pub name: String,
}

/// A repository with the places it is checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositorySummary {
    pub repository: Repository,
    pub checkouts: Vec<Checkout>,
}

#[async_trait]
pub trait RunnerRepository: Send + Sync {
    /// The runner with this name, created as `candidate` with its change when it does not exist.
    async fn ensure(&self, candidate: &Runner) -> Result<(Runner, Option<Change>), AppError>;
    async fn get(&self, id: Id<Runner>) -> Result<Option<Runner>, AppError>;
    /// By name.
    async fn list(&self) -> Result<Vec<Runner>, AppError>;
}

#[async_trait]
pub trait CheckoutRepository: Send + Sync {
    /// The checkout with this key on `runner`, with its repository.
    async fn find(
        &self,
        runner: Id<Runner>,
        key: &CheckoutPath,
    ) -> Result<Option<(Repository, Checkout)>, AppError>;
    /// Fails with [`checkout_exists`] when the runner has a checkout with the same key.
    async fn insert(
        &self,
        repository: &Repository,
        checkout: &Checkout,
    ) -> Result<Change, AppError>;
    /// By name, each with its checkouts.
    async fn list(&self) -> Result<Vec<RepositorySummary>, AppError>;
}

#[must_use]
pub fn path_missing() -> AppError {
    AppError::invalid("path-missing", "the path is empty")
}

#[must_use]
pub fn path_relative(path: &str) -> AppError {
    AppError::invalid("path-relative", format!("{path} is not an absolute path"))
}

/// A path to another machine or a device, refused before any file call touches it.
#[must_use]
pub fn network_path_refused(path: &str) -> AppError {
    AppError::invalid(
        "network-path-refused",
        format!(
            "{path} reaches another machine or a device, list its host under network_shares in the config file to allow it"
        ),
    )
}

#[must_use]
pub fn not_a_git_project(path: &str) -> AppError {
    AppError::invalid(
        "not-a-git-project",
        format!("{path} is not inside a git repository"),
    )
}

#[must_use]
pub fn runner_not_found(id: Id<Runner>) -> AppError {
    AppError::not_found("runner-not-found", format!("runner {id} does not exist"))
}

#[must_use]
pub fn runner_unreachable(id: Id<Runner>) -> AppError {
    AppError::unavailable(
        "runner-unreachable",
        format!("runner {id} is not connected to this daemon"),
    )
}

#[must_use]
pub fn checkout_exists(key: &str) -> AppError {
    AppError::conflict(
        "checkout-exists",
        format!("the runner already has a checkout at {key}"),
    )
}

fn invalid(error: &RepositoryError) -> AppError {
    let (text, empty) = match error {
        RepositoryError::RunnerName(text) => (text, "runner-name-empty"),
        RepositoryError::Name(text) => (text, "repository-name-empty"),
        RepositoryError::Path(text) => (text, "path-missing"),
    };
    AppError::text(*text, empty, error.to_string())
}

pub struct RunnerService {
    runners: Arc<dyn RunnerRepository>,
    checkouts: Arc<dyn CheckoutRepository>,
    port: Arc<dyn RunnerPort>,
    local: Runner,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for RunnerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerService").finish_non_exhaustive()
    }
}

impl RunnerService {
    /// Records the daemon's own machine as the runner [`LOCAL_RUNNER`] that `port` drives.
    pub async fn start(
        runners: Arc<dyn RunnerRepository>,
        checkouts: Arc<dyn CheckoutRepository>,
        port: Arc<dyn RunnerPort>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Result<Self, AppError> {
        let name = RunnerName::parse(LOCAL_RUNNER).map_err(|error| invalid(&error))?;
        let candidate = Runner::new(ids.next(), name, RunnerOs::current(), clock.now());
        let (local, change) = runners.ensure(&candidate).await?;
        if let Some(change) = change {
            signal.notify(&change);
        }
        Ok(Self {
            runners,
            checkouts,
            port,
            local,
            clock,
            ids,
            signal,
        })
    }

    /// The runner on the daemon's own machine.
    #[must_use]
    pub const fn local(&self) -> &Runner {
        &self.local
    }

    pub async fn list(&self) -> Result<Vec<Runner>, AppError> {
        self.runners.list().await
    }

    pub async fn repositories(&self) -> Result<Vec<RepositorySummary>, AppError> {
        self.checkouts.list().await
    }

    /// The port driving `runner`; only the local runner is connected.
    async fn port(&self, runner: Id<Runner>) -> Result<&dyn RunnerPort, AppError> {
        if runner != self.local.id() {
            return Err(match self.runners.get(runner).await? {
                Some(_) => runner_unreachable(runner),
                None => runner_not_found(runner),
            });
        }
        Ok(self.port.as_ref())
    }

    /// The repository `path` on `runner` belongs to.
    pub async fn locate(&self, runner: Id<Runner>, path: &str) -> Result<Located, AppError> {
        self.port(runner).await?.locate(path).await
    }

    /// Where `path` on `runner` is, with the repository recorded for that checkout, if any.
    pub(crate) async fn recorded(
        &self,
        runner: Id<Runner>,
        path: &str,
    ) -> Result<(Located, Option<Repository>), AppError> {
        let located = self.locate(runner, path).await?;
        let key = CheckoutPath::parse(&located.key).map_err(|error| invalid(&error))?;
        let found = self.checkouts.find(runner, &key).await?;
        Ok((located, found.map(|(repository, _)| repository)))
    }

    pub(crate) async fn create_worktree(
        &self,
        runner: Id<Runner>,
        request: &NewWorktree,
    ) -> Result<CreatedWorktree, AppError> {
        self.port(runner).await?.create_worktree(request).await
    }

    pub(crate) async fn ensure_worktree(
        &self,
        runner: Id<Runner>,
        request: &EnsureWorktree,
    ) -> Result<EnsuredWorktree, AppError> {
        self.port(runner).await?.ensure_worktree(request).await
    }

    pub(crate) async fn remove_worktree(
        &self,
        runner: Id<Runner>,
        request: &RemoveWorktree,
    ) -> Result<WorktreeRemoval, AppError> {
        self.port(runner).await?.remove_worktree(request).await
    }

    pub(crate) async fn read_transcript(
        &self,
        runner: Id<Runner>,
        request: &TranscriptRead,
    ) -> Result<Option<TranscriptChunk>, AppError> {
        self.port(runner).await?.read_transcript(request).await
    }

    pub(crate) async fn transcript_facts(
        &self,
        runner: Id<Runner>,
        source: &TranscriptSource,
    ) -> Result<Option<TranscriptSummary>, AppError> {
        self.port(runner).await?.transcript_facts(source).await
    }

    pub(crate) async fn session_changes(
        &self,
        runner: Id<Runner>,
        request: &ChangeRequest,
    ) -> Result<SessionChanges, AppError> {
        self.port(runner).await?.session_changes(request).await
    }

    pub(crate) async fn session_diff(
        &self,
        runner: Id<Runner>,
        request: &DiffRequest,
    ) -> Result<FileDiff, AppError> {
        self.port(runner).await?.session_diff(request).await
    }

    pub(crate) async fn review_changes(
        &self,
        runner: Id<Runner>,
        request: &ReviewRequest,
    ) -> Result<BranchChanges, AppError> {
        self.port(runner).await?.review_changes(request).await
    }

    pub(crate) async fn review_patch(
        &self,
        runner: Id<Runner>,
        request: &ReviewRequest,
        path: &str,
    ) -> Result<FilePatch, AppError> {
        self.port(runner).await?.review_patch(request, path).await
    }

    pub(crate) async fn review_file(
        &self,
        runner: Id<Runner>,
        root: &str,
        branch: &str,
        path: &str,
    ) -> Result<BranchFile, AppError> {
        self.port(runner)
            .await?
            .review_file(root, branch, path)
            .await
    }

    /// Root of the repository's checkout on the runner, if it has one.
    pub(crate) async fn root(
        &self,
        runner: Id<Runner>,
        repository: Id<Repository>,
    ) -> Result<Option<String>, AppError> {
        let summaries = self.checkouts.list().await?;
        Ok(summaries
            .into_iter()
            .filter(|summary| summary.repository.id() == repository)
            .flat_map(|summary| summary.checkouts)
            .find(|checkout| checkout.runner() == runner)
            .map(|checkout| checkout.path().as_str().to_owned()))
    }

    pub(crate) async fn folders_exist(
        &self,
        runner: Id<Runner>,
        paths: &[String],
    ) -> Result<Vec<bool>, AppError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        self.port(runner).await?.folders_exist(paths).await
    }

    /// Key of the checkout `path` is in, or `None` when it is in no repository.
    pub(crate) async fn key(
        &self,
        runner: Id<Runner>,
        path: &str,
    ) -> Result<Option<CheckoutPath>, AppError> {
        match self.locate(runner, path).await {
            Ok(located) => Ok(Some(
                CheckoutPath::parse(&located.key).map_err(|error| invalid(&error))?,
            )),
            Err(error) if error.code() == "not-a-git-project" => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The repository checked out at `path` on `runner`, recorded on first sight.
    pub(crate) async fn checkout(
        &self,
        runner: Id<Runner>,
        path: &str,
    ) -> Result<Repository, AppError> {
        let located = self.locate(runner, path).await?;
        let key = CheckoutPath::parse(&located.key).map_err(|error| invalid(&error))?;
        if let Some((repository, _)) = self.checkouts.find(runner, &key).await? {
            return Ok(repository);
        }
        let now = self.clock.now();
        let name = RepositoryName::from_folder(&located.name)
            .or_else(|_| RepositoryName::from_folder(&located.root))
            .map_err(|error| invalid(&error))?;
        let repository = Repository::new(self.ids.next(), name, now);
        let root = CheckoutPath::parse(&located.root).map_err(|error| invalid(&error))?;
        let checkout = Checkout::new(self.ids.next(), repository.id(), runner, root, key, now);
        match self.checkouts.insert(&repository, &checkout).await {
            Ok(change) => {
                self.signal.notify(&change);
                Ok(repository)
            }
            // someone recorded the same checkout meanwhile
            Err(error) if error.kind() == ErrorKind::Conflict => self
                .checkouts
                .find(runner, checkout.key())
                .await?
                .map(|(repository, _)| repository)
                .ok_or(error),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use nullradix_domain::TextError;

    use super::*;

    #[test]
    fn maps_validation_errors_to_stable_codes() {
        assert_eq!(
            invalid(&RepositoryError::Path(TextError::Empty)).code(),
            "path-missing"
        );
        assert_eq!(
            invalid(&RepositoryError::Name(TextError::TooLong(200))).code(),
            "value-too-long"
        );
        assert_eq!(path_relative("x").code(), "path-relative");
        assert_eq!(not_a_git_project("/x").kind(), ErrorKind::Invalid);
        let id = Id::from_uuid(uuid::Uuid::from_u128(1));
        assert_eq!(runner_not_found(id).kind(), ErrorKind::NotFound);
        assert_eq!(checkout_exists("/x").kind(), ErrorKind::Conflict);
    }
}
