//! Boards group a project's tickets; actors record who changes them.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Actor, ActorKind, ActorName, Board, BoardError, BoardName, Id, Project, Timestamp, Version,
};

use crate::projects::{self, ProjectRepository};
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

#[async_trait]
pub trait ActorRepository: Send + Sync {
    /// The actor of this kind and name, created as `candidate` when it does not exist yet.
    async fn ensure(&self, candidate: &Actor) -> Result<Actor, AppError>;
}

/// A board with what lists show next to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardSummary {
    pub board: Board,
    pub project_key: String,
    pub owner_name: String,
    /// Tickets that are neither done nor not needed.
    pub open_tickets: u32,
}

#[async_trait]
pub trait BoardRepository: Send + Sync {
    /// Fails with [`crate::projects::not_found`] when the project does not exist.
    async fn insert(&self, board: &Board) -> Result<Change, AppError>;
    /// Boards of `project`, or of all projects, by project key and then name.
    async fn list(&self, project: Option<Id<Project>>) -> Result<Vec<BoardSummary>, AppError>;
    async fn get(&self, id: Id<Board>) -> Result<Option<BoardSummary>, AppError>;
    async fn update(&self, board: &Board, expected: Version) -> Result<Change, AppError>;
    async fn delete(
        &self,
        id: Id<Board>,
        expected: Version,
        at: Timestamp,
    ) -> Result<Change, AppError>;
}

fn invalid(error: &BoardError) -> AppError {
    let code = match error {
        BoardError::InvalidName => "invalid-board-name",
        BoardError::InvalidActorName => "invalid-actor-name",
    };
    AppError::invalid(code, error.to_string())
}

#[must_use]
pub fn not_found(id: Id<Board>) -> AppError {
    AppError::not_found("board-not-found", format!("board {id} does not exist"))
}

/// The person at this machine, as the actor every token and browser session acts for.
pub async fn local_person(
    actors: &dyn ActorRepository,
    name: &str,
    ids: &dyn IdGenerator,
    clock: &dyn Clock,
) -> Result<Actor, AppError> {
    let name = ActorName::parse(name).map_err(|error| invalid(&error))?;
    let candidate = Actor::new(
        Id::from_uuid(ids.next_uuid()),
        ActorKind::Human,
        name,
        clock.now(),
    );
    actors.ensure(&candidate).await
}

pub struct BoardService {
    repository: Arc<dyn BoardRepository>,
    projects: Arc<dyn ProjectRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for BoardService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardService").finish_non_exhaustive()
    }
}

impl BoardService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn BoardRepository>,
        projects: Arc<dyn ProjectRepository>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            repository,
            projects,
            clock,
            ids,
            signal,
        }
    }

    pub async fn create(
        &self,
        project: Id<Project>,
        name: &str,
        owner: &Actor,
    ) -> Result<BoardSummary, AppError> {
        let name = BoardName::parse(name).map_err(|error| invalid(&error))?;
        let board = Board::new(self.ids.next(), project, name, owner.id(), self.clock.now());
        let change = self.repository.insert(&board).await?;
        self.signal.notify(&change);
        self.repository
            .get(board.id())
            .await?
            .ok_or_else(|| not_found(board.id()))
    }

    /// Boards of `project`, which must exist, or of all projects.
    pub async fn list(&self, project: Option<Id<Project>>) -> Result<Vec<BoardSummary>, AppError> {
        let boards = self.repository.list(project).await?;
        if let Some(project) = project
            && boards.is_empty()
            && self.projects.get(project).await?.is_none()
        {
            return Err(projects::not_found(project));
        }
        Ok(boards)
    }

    /// The board, which must belong to `project`.
    pub async fn get(&self, project: Id<Project>, id: Id<Board>) -> Result<BoardSummary, AppError> {
        match self.repository.get(id).await? {
            Some(summary) if summary.board.project() == project => Ok(summary),
            _ if self.projects.get(project).await?.is_none() => Err(projects::not_found(project)),
            _ => Err(not_found(id)),
        }
    }

    /// Renames the board; the same name returns it unchanged.
    pub async fn rename(
        &self,
        project: Id<Project>,
        id: Id<Board>,
        name: &str,
        expected: Version,
    ) -> Result<BoardSummary, AppError> {
        let name = BoardName::parse(name).map_err(|error| invalid(&error))?;
        let current = self.get(project, id).await?;
        if current.board.version() != expected {
            return Err(AppError::version_conflict("board", id));
        }
        let Some(renamed) = current.board.renamed(name, self.clock.now()) else {
            return Ok(current);
        };
        let change = self.repository.update(&renamed, expected).await?;
        self.signal.notify(&change);
        Ok(BoardSummary {
            board: renamed,
            ..current
        })
    }

    pub async fn delete(
        &self,
        project: Id<Project>,
        id: Id<Board>,
        expected: Version,
    ) -> Result<(), AppError> {
        self.get(project, id).await?;
        let change = self
            .repository
            .delete(id, expected, self.clock.now())
            .await?;
        self.signal.notify(&change);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_validation_errors_to_stable_codes() {
        assert_eq!(
            invalid(&BoardError::InvalidName).code(),
            "invalid-board-name"
        );
        assert_eq!(
            invalid(&BoardError::InvalidActorName).code(),
            "invalid-actor-name"
        );
        let id = Id::from_uuid(uuid::Uuid::from_u128(1));
        assert_eq!(not_found(id).code(), "board-not-found");
    }
}
