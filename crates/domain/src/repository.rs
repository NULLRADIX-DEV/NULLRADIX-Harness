use strum::{EnumIter, EnumString, IntoStaticStr};

use crate::{Id, TextError, Timestamp, Version, required_text, utf16_prefix};

pub const RUNNER_NAME_MAX: usize = 100;
pub const REPOSITORY_NAME_MAX: usize = 200;
pub const CHECKOUT_PATH_MAX: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepositoryError {
    #[error("the runner name {0}")]
    RunnerName(TextError),
    #[error("the repository name {0}")]
    Name(TextError),
    #[error("the path {0}")]
    Path(TextError),
}

/// Operating system of a runner; paths follow its rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum RunnerOs {
    Windows,
    Linux,
}

impl RunnerOs {
    /// The system this code was built for.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunnerName(String);

impl RunnerName {
    /// Names are unique among runners.
    pub fn parse(text: &str) -> Result<Self, RepositoryError> {
        required_text(text, RUNNER_NAME_MAX)
            .map(|name| Self(name.to_owned()))
            .map_err(RepositoryError::RunnerName)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Every field of a stored runner, for persistence adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerRecord {
    pub id: Id<Runner>,
    pub name: RunnerName,
    pub os: RunnerOs,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

/// A machine that checks out repositories and runs processes in them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runner(RunnerRecord);

impl Runner {
    #[must_use]
    pub const fn new(id: Id<Self>, name: RunnerName, os: RunnerOs, now: Timestamp) -> Self {
        Self(RunnerRecord {
            id,
            name,
            os,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        })
    }

    #[must_use]
    pub const fn restore(record: RunnerRecord) -> Self {
        Self(record)
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.0.id
    }

    #[must_use]
    pub const fn name(&self) -> &RunnerName {
        &self.0.name
    }

    #[must_use]
    pub const fn os(&self) -> RunnerOs {
        self.0.os
    }

    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.0.created_at
    }

    #[must_use]
    pub const fn updated_at(&self) -> Timestamp {
        self.0.updated_at
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.0.version
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryName(String);

impl RepositoryName {
    pub fn parse(text: &str) -> Result<Self, RepositoryError> {
        required_text(text, REPOSITORY_NAME_MAX)
            .map(|name| Self(name.to_owned()))
            .map_err(RepositoryError::Name)
    }

    /// A name from a folder name, cut to the longest allowed start.
    pub fn from_folder(folder: &str) -> Result<Self, RepositoryError> {
        Self::parse(&utf16_prefix(folder.trim(), REPOSITORY_NAME_MAX))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Every field of a stored repository, for persistence adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRecord {
    pub id: Id<Repository>,
    pub name: RepositoryName,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

/// A git repository, checked out on one or more runners.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository(RepositoryRecord);

impl Repository {
    #[must_use]
    pub const fn new(id: Id<Self>, name: RepositoryName, now: Timestamp) -> Self {
        Self(RepositoryRecord {
            id,
            name,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        })
    }

    #[must_use]
    pub const fn restore(record: RepositoryRecord) -> Self {
        Self(record)
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.0.id
    }

    #[must_use]
    pub const fn name(&self) -> &RepositoryName {
        &self.0.name
    }

    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.0.created_at
    }

    #[must_use]
    pub const fn updated_at(&self) -> Timestamp {
        self.0.updated_at
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.0.version
    }
}

/// A folder path on a runner, as given or as its comparable key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CheckoutPath(String);

impl CheckoutPath {
    pub fn parse(text: &str) -> Result<Self, RepositoryError> {
        required_text(text, CHECKOUT_PATH_MAX)
            .map(|path| Self(path.to_owned()))
            .map_err(RepositoryError::Path)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Every field of a stored checkout, for persistence adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRecord {
    pub id: Id<Checkout>,
    pub repository: Id<Repository>,
    pub runner: Id<Runner>,
    pub path: CheckoutPath,
    pub key: CheckoutPath,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

/// Where a repository lives on a runner: one per repository and runner, one per folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout(CheckoutRecord);

impl Checkout {
    #[must_use]
    pub const fn new(
        id: Id<Self>,
        repository: Id<Repository>,
        runner: Id<Runner>,
        path: CheckoutPath,
        key: CheckoutPath,
        now: Timestamp,
    ) -> Self {
        Self(CheckoutRecord {
            id,
            repository,
            runner,
            path,
            key,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        })
    }

    #[must_use]
    pub const fn restore(record: CheckoutRecord) -> Self {
        Self(record)
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.0.id
    }

    #[must_use]
    pub const fn repository(&self) -> Id<Repository> {
        self.0.repository
    }

    #[must_use]
    pub const fn runner(&self) -> Id<Runner> {
        self.0.runner
    }

    #[must_use]
    pub const fn path(&self) -> &CheckoutPath {
        &self.0.path
    }

    /// Comparable spelling of the path; equal keys mean the same folder.
    #[must_use]
    pub const fn key(&self) -> &CheckoutPath {
        &self.0.key
    }

    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.0.created_at
    }

    #[must_use]
    pub const fn updated_at(&self) -> Timestamp {
        self.0.updated_at
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.0.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id<T>(value: u128) -> Id<T> {
        Id::from_uuid(uuid::Uuid::from_u128(value))
    }

    #[test]
    fn names_and_paths_are_trimmed_and_limited() {
        assert_eq!(RunnerName::parse(" local ").unwrap().as_str(), "local");
        assert_eq!(
            RunnerName::parse(&"a".repeat(101)),
            Err(RepositoryError::RunnerName(TextError::TooLong(100)))
        );
        assert_eq!(
            RepositoryName::parse(" "),
            Err(RepositoryError::Name(TextError::Empty))
        );
        assert_eq!(
            CheckoutPath::parse(&"a".repeat(1001)),
            Err(RepositoryError::Path(TextError::TooLong(1000)))
        );
        assert_eq!(
            CheckoutPath::parse(" /work/x ").unwrap().as_str(),
            "/work/x"
        );
    }

    #[test]
    fn folder_names_are_cut_to_fit() {
        let long = "\u{1f600}".repeat(150);
        let name = RepositoryName::from_folder(&long).unwrap();
        assert_eq!(name.as_str(), "\u{1f600}".repeat(100));
        assert_eq!(
            RepositoryName::from_folder(" ").unwrap_err(),
            RepositoryError::Name(TextError::Empty)
        );
    }

    #[test]
    fn new_entities_start_at_the_first_version_and_restore_keeps_every_field() {
        let now = Timestamp::from_millis(7);
        let runner = Runner::new(
            id(1),
            RunnerName::parse("local").unwrap(),
            RunnerOs::Linux,
            now,
        );
        assert_eq!(runner.version(), Version::INITIAL);
        assert_eq!(runner.os().as_str(), "linux");
        assert_eq!("windows".parse::<RunnerOs>(), Ok(RunnerOs::Windows));
        let repository = Repository::new(id(2), RepositoryName::parse("arches").unwrap(), now);
        let checkout = Checkout::new(
            id(3),
            repository.id(),
            runner.id(),
            CheckoutPath::parse("C:/Work/Arches").unwrap(),
            CheckoutPath::parse("c:/work/arches").unwrap(),
            now,
        );
        assert_eq!(checkout.key().as_str(), "c:/work/arches");
        assert_eq!(checkout.updated_at(), now);
        let restored = Checkout::restore(CheckoutRecord {
            id: checkout.id(),
            repository: checkout.repository(),
            runner: checkout.runner(),
            path: checkout.path().clone(),
            key: checkout.key().clone(),
            created_at: checkout.created_at(),
            updated_at: checkout.updated_at(),
            version: checkout.version(),
        });
        assert_eq!(restored, checkout);
    }
}
