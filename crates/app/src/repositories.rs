//! Runners check out repositories; boards point to the repository their work happens in.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Checkout, CheckoutPath, Id, Repository, RepositoryError, RepositoryName, Runner, RunnerName,
    RunnerOs,
};

use crate::runs::RunnerPort;
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

    /// The repository `path` on `runner` belongs to.
    pub async fn locate(&self, runner: Id<Runner>, path: &str) -> Result<Located, AppError> {
        if runner != self.local.id() {
            return Err(match self.runners.get(runner).await? {
                Some(_) => runner_unreachable(runner),
                None => runner_not_found(runner),
            });
        }
        self.port.locate(path).await
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
