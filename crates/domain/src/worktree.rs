use strum::{EnumIter, EnumString, IntoStaticStr};

use crate::{
    Id, Repository, Runner, TargetBranch, TextError, Ticket, TicketKey, Timestamp, Version,
    required_text,
};

pub const WORKTREE_BRANCH_MAX: usize = 400;
pub const WORKTREE_PATH_MAX: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorktreeError {
    #[error("the branch {0}")]
    Branch(TextError),
    #[error("the worktree path {0}")]
    Path(TextError),
}

/// The branch a worktree has checked out, such as `ticket/ARC-0007-fix-it`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorktreeBranch(String);

impl WorktreeBranch {
    pub fn parse(text: &str) -> Result<Self, WorktreeError> {
        required_text(text, WORKTREE_BRANCH_MAX)
            .map(|branch| Self(branch.to_owned()))
            .map_err(WorktreeError::Branch)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The folder of a worktree on its runner, with forward slashes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorktreePath(String);

impl WorktreePath {
    pub fn parse(text: &str) -> Result<Self, WorktreeError> {
        required_text(text, WORKTREE_PATH_MAX)
            .map(|path| Self(path.to_owned()))
            .map_err(WorktreeError::Path)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a worktree exists; research and plan worktrees are detached and temporary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum WorktreePurpose {
    Work,
    Research,
    Plan,
}

impl WorktreePurpose {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Whether the folder should exist; the runner brings the actual state there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum DesiredState {
    Present,
    Absent,
}

impl DesiredState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// What the runner last reported about the folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum ActualState {
    Unknown,
    Creating,
    Present,
    Removing,
    Removed,
    Failed,
}

impl ActualState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// The setup commands of a new worktree; `none` when nothing is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum SetupState {
    #[strum(serialize = "none")]
    Unset,
    Pending,
    Done,
    Failed,
}

impl SetupState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// What a new worktree brings; the states start as nothing happened yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeDraft {
    pub repository: Id<Repository>,
    pub runner: Id<Runner>,
    pub branch: WorktreeBranch,
    pub target_branch: Option<TargetBranch>,
    pub path: WorktreePath,
    pub purpose: WorktreePurpose,
}

/// Every stored field of a worktree, for persistence adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRecord {
    pub id: Id<Worktree>,
    pub repository: Id<Repository>,
    pub runner: Id<Runner>,
    pub branch: WorktreeBranch,
    pub target_branch: Option<TargetBranch>,
    pub path: WorktreePath,
    pub purpose: WorktreePurpose,
    pub desired: DesiredState,
    pub actual: ActualState,
    pub setup: SetupState,
    pub ready_to_merge: bool,
    pub last_error: Option<String>,
    /// When the runner last reported the actual state.
    pub observed_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

/// A registry entry of legacy `worktrees.json`: it stays, even after its folder is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree(WorktreeRecord);

impl Worktree {
    #[must_use]
    pub fn new(id: Id<Self>, draft: WorktreeDraft, now: Timestamp) -> Self {
        Self(WorktreeRecord {
            id,
            repository: draft.repository,
            runner: draft.runner,
            branch: draft.branch,
            target_branch: draft.target_branch,
            path: draft.path,
            purpose: draft.purpose,
            desired: DesiredState::Present,
            actual: ActualState::Unknown,
            setup: SetupState::Unset,
            ready_to_merge: false,
            last_error: None,
            observed_at: None,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        })
    }

    #[must_use]
    pub const fn restore(record: WorktreeRecord) -> Self {
        Self(record)
    }

    #[must_use]
    pub const fn record(&self) -> &WorktreeRecord {
        &self.0
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.0.id
    }

    #[must_use]
    pub const fn branch(&self) -> &WorktreeBranch {
        &self.0.branch
    }

    #[must_use]
    pub const fn path(&self) -> &WorktreePath {
        &self.0.path
    }
}

/// A ticket in a worktree's list; the list never shrinks, a link only ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeTicket {
    pub ticket: Id<Ticket>,
    pub key: TicketKey,
    /// Join order; 0 owns the worktree.
    pub position: u32,
    pub joined_at: Timestamp,
    /// When the ticket's branch and worktree were cleared.
    pub released_at: Option<Timestamp>,
}

impl WorktreeTicket {
    /// Whether the ticket still shows this worktree as its branch and folder.
    #[must_use]
    pub const fn live(&self) -> bool {
        self.released_at.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id<T>(value: u128) -> Id<T> {
        Id::from_uuid(uuid::Uuid::from_u128(value))
    }

    #[test]
    fn branches_and_paths_are_trimmed_and_limited() {
        assert_eq!(
            WorktreeBranch::parse(" ticket/ARC-0001-a ")
                .unwrap()
                .as_str(),
            "ticket/ARC-0001-a"
        );
        assert_eq!(
            WorktreeBranch::parse(&"b".repeat(401)),
            Err(WorktreeError::Branch(TextError::TooLong(400)))
        );
        assert_eq!(
            WorktreePath::parse(" "),
            Err(WorktreeError::Path(TextError::Empty))
        );
        assert_eq!(
            WorktreePath::parse(&"p".repeat(501)),
            Err(WorktreeError::Path(TextError::TooLong(500)))
        );
    }

    #[test]
    fn states_keep_their_stored_names() {
        assert_eq!(SetupState::Unset.as_str(), "none");
        assert_eq!("none".parse::<SetupState>(), Ok(SetupState::Unset));
        assert_eq!(ActualState::Removing.as_str(), "removing");
        assert_eq!("absent".parse::<DesiredState>(), Ok(DesiredState::Absent));
        assert_eq!(WorktreePurpose::Research.as_str(), "research");
    }

    #[test]
    fn new_worktrees_are_wanted_and_not_yet_seen() {
        let draft = WorktreeDraft {
            repository: id(1),
            runner: id(2),
            branch: WorktreeBranch::parse("ticket/ARC-0001-a").unwrap(),
            target_branch: None,
            path: WorktreePath::parse("/work/arches-worktrees/ARC-0001").unwrap(),
            purpose: WorktreePurpose::Work,
        };
        let worktree = Worktree::new(id(3), draft, Timestamp::from_millis(4));
        let record = worktree.record();
        assert_eq!(
            (record.desired, record.actual, record.setup),
            (
                DesiredState::Present,
                ActualState::Unknown,
                SetupState::Unset
            )
        );
        assert!(!record.ready_to_merge);
        assert_eq!(record.version, Version::INITIAL);
        assert_eq!(Worktree::restore(record.clone()), worktree);
    }
}
