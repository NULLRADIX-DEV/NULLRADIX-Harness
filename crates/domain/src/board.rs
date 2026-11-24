use strum::{EnumIter, EnumString, IntoStaticStr};

use crate::{Id, Project, Repository, TextError, Timestamp, Version, required_text};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BoardError {
    #[error("the board name {0}")]
    Name(TextError),
    #[error("the actor name {0}")]
    ActorName(TextError),
}

/// Who acted: the person, a Claude session, a subagent, the dispatcher or the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum ActorKind {
    Human,
    Session,
    Subagent,
    Dispatcher,
    System,
}

impl ActorKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ActorName(String);

impl ActorName {
    pub fn parse(text: &str) -> Result<Self, BoardError> {
        required_text(text, 100)
            .map(|name| Self(name.to_owned()))
            .map_err(BoardError::ActorName)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Anyone who changes the board; one row per kind and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    id: Id<Self>,
    kind: ActorKind,
    name: ActorName,
    created_at: Timestamp,
}

impl Actor {
    #[must_use]
    pub const fn new(
        id: Id<Self>,
        kind: ActorKind,
        name: ActorName,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            kind,
            name,
            created_at,
        }
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.id
    }

    #[must_use]
    pub const fn kind(&self) -> ActorKind {
        self.kind
    }

    #[must_use]
    pub const fn name(&self) -> &ActorName {
        &self.name
    }

    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardName(String);

impl BoardName {
    /// Names need not be unique.
    pub fn parse(text: &str) -> Result<Self, BoardError> {
        required_text(text, 200)
            .map(|name| Self(name.to_owned()))
            .map_err(BoardError::Name)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Every field of a stored board, for persistence adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardRecord {
    pub id: Id<Board>,
    pub project: Id<Project>,
    pub name: BoardName,
    pub owner: Id<Actor>,
    pub dispatch_enabled: bool,
    pub repository: Option<Id<Repository>>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

/// A set of tickets inside a project, owned by the person who created it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    id: Id<Self>,
    project: Id<Project>,
    name: BoardName,
    owner: Id<Actor>,
    dispatch_enabled: bool,
    repository: Option<Id<Repository>>,
    created_at: Timestamp,
    updated_at: Timestamp,
    version: Version,
}

impl Board {
    #[must_use]
    pub const fn new(
        id: Id<Self>,
        project: Id<Project>,
        name: BoardName,
        owner: Id<Actor>,
        now: Timestamp,
    ) -> Self {
        Self {
            id,
            project,
            name,
            owner,
            dispatch_enabled: true,
            repository: None,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        }
    }

    #[must_use]
    pub fn restore(record: BoardRecord) -> Self {
        Self {
            id: record.id,
            project: record.project,
            name: record.name,
            owner: record.owner,
            dispatch_enabled: record.dispatch_enabled,
            repository: record.repository,
            created_at: record.created_at,
            updated_at: record.updated_at,
            version: record.version,
        }
    }

    /// The renamed board, or `None` when the name does not change.
    #[must_use]
    pub fn renamed(&self, name: BoardName, now: Timestamp) -> Option<Self> {
        (name != self.name).then(|| self.changed(now, |board| board.name = name))
    }

    /// The board with dispatching switched, or `None` when it already is.
    #[must_use]
    pub fn with_dispatch(&self, enabled: bool, now: Timestamp) -> Option<Self> {
        (enabled != self.dispatch_enabled)
            .then(|| self.changed(now, |board| board.dispatch_enabled = enabled))
    }

    /// The board pointing to `repository`, or `None` when it already does.
    #[must_use]
    pub fn with_repository(
        &self,
        repository: Option<Id<Repository>>,
        now: Timestamp,
    ) -> Option<Self> {
        (repository != self.repository)
            .then(|| self.changed(now, |board| board.repository = repository))
    }

    fn changed(&self, now: Timestamp, change: impl FnOnce(&mut Self)) -> Self {
        let mut next = self.clone();
        change(&mut next);
        next.updated_at = now.max(self.updated_at);
        next.version = self.version.next();
        next
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.id
    }

    #[must_use]
    pub const fn project(&self) -> Id<Project> {
        self.project
    }

    #[must_use]
    pub const fn name(&self) -> &BoardName {
        &self.name
    }

    #[must_use]
    pub const fn owner(&self) -> Id<Actor> {
        self.owner
    }

    #[must_use]
    pub const fn dispatch_enabled(&self) -> bool {
        self.dispatch_enabled
    }

    /// The repository the board's work happens in, if any.
    #[must_use]
    pub const fn repository(&self) -> Option<Id<Repository>> {
        self.repository
    }

    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }

    #[must_use]
    pub const fn updated_at(&self) -> Timestamp {
        self.updated_at
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Board {
        Board::new(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            Id::from_uuid(uuid::Uuid::from_u128(2)),
            BoardName::parse("Main").unwrap(),
            Id::from_uuid(uuid::Uuid::from_u128(3)),
            Timestamp::from_millis(10),
        )
    }

    #[test]
    fn names_are_trimmed_and_counted_in_utf16_units() {
        assert_eq!(BoardName::parse("  Main ").unwrap().as_str(), "Main");
        assert_eq!(
            BoardName::parse(" "),
            Err(BoardError::Name(TextError::Empty))
        );
        assert!(BoardName::parse(&"\u{1f600}".repeat(100)).is_ok());
        assert_eq!(
            BoardName::parse(&"\u{1f600}".repeat(101)),
            Err(BoardError::Name(TextError::TooLong(200)))
        );
        assert_eq!(ActorName::parse(" anna ").unwrap().as_str(), "anna");
        assert_eq!(
            ActorName::parse(&"a".repeat(101)),
            Err(BoardError::ActorName(TextError::TooLong(100)))
        );
        assert_eq!(ActorKind::Subagent.as_str(), "subagent");
    }

    #[test]
    fn changes_bump_the_version_and_no_ops_return_none() {
        let board = board();
        assert!(board.dispatch_enabled());
        assert_eq!(board.version(), Version::INITIAL);
        let main = BoardName::parse("Main").unwrap();
        assert_eq!(board.renamed(main, Timestamp::from_millis(20)), None);
        let renamed = board
            .renamed(BoardName::parse("Next").unwrap(), Timestamp::from_millis(5))
            .unwrap();
        assert_eq!(renamed.name().as_str(), "Next");
        assert_eq!(renamed.version(), Version::INITIAL.next());
        assert_eq!(renamed.updated_at(), Timestamp::from_millis(10));
        assert_eq!(board.with_dispatch(true, Timestamp::from_millis(20)), None);
        let paused = board
            .with_dispatch(false, Timestamp::from_millis(20))
            .unwrap();
        assert!(!paused.dispatch_enabled());
        assert_eq!(paused.updated_at(), Timestamp::from_millis(20));
    }

    #[test]
    fn restore_keeps_every_field() {
        let board = board();
        let restored = Board::restore(BoardRecord {
            id: board.id(),
            project: board.project(),
            name: board.name().clone(),
            owner: board.owner(),
            dispatch_enabled: board.dispatch_enabled(),
            repository: board.repository(),
            created_at: board.created_at(),
            updated_at: board.updated_at(),
            version: board.version(),
        });
        assert_eq!(restored, board);
    }

    #[test]
    fn pointing_to_a_repository_bumps_the_version_once() {
        let board = board();
        assert_eq!(board.repository(), None);
        assert_eq!(
            board.with_repository(None, Timestamp::from_millis(20)),
            None
        );
        let repository = Id::from_uuid(uuid::Uuid::from_u128(5));
        let bound = board
            .with_repository(Some(repository), Timestamp::from_millis(20))
            .unwrap();
        assert_eq!(bound.repository(), Some(repository));
        assert_eq!(bound.version(), Version::INITIAL.next());
        assert_eq!(bound.updated_at(), Timestamp::from_millis(20));
        assert_eq!(
            bound.with_repository(Some(repository), Timestamp::from_millis(30)),
            None
        );
        let cleared = bound
            .with_repository(None, Timestamp::from_millis(30))
            .unwrap();
        assert_eq!(cleared.repository(), None);
        assert_eq!(cleared.version(), Version::INITIAL.next().next());
    }
}
