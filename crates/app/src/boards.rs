//! Boards group a project's tickets; actors record who changes them.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Actor, ActorKind, ActorName, Board, BoardError, BoardName, CheckoutPath, Id, Project,
    Repository, Runner, Ticket, TicketKey, Version,
};

use crate::projects::{self, ProjectRepository};
use crate::repositories::RunnerService;
use crate::tickets::{self, TicketRepository};
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
    /// Name of the repository the board points to.
    pub repository_name: Option<String>,
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
    /// Boards pointing to the repository checked out on `runner` at `key`.
    async fn list_by_checkout(
        &self,
        runner: Id<Runner>,
        key: &CheckoutPath,
    ) -> Result<Vec<BoardSummary>, AppError>;
}

fn invalid(error: &BoardError) -> AppError {
    let (text, empty) = match error {
        BoardError::Name(text) => (text, "board-name-empty"),
        BoardError::ActorName(text) => (text, "actor-name-empty"),
    };
    AppError::text(*text, empty, error.to_string())
}

#[must_use]
pub fn not_found(id: Id<Board>) -> AppError {
    AppError::not_found("board-not-found", format!("board {id} does not exist"))
}

/// The error for a board that is missing from `project`: the project itself, or the board.
pub(crate) async fn missing(
    projects: &dyn ProjectRepository,
    project: Id<Project>,
    board: Id<Board>,
) -> AppError {
    match projects.get(project).await {
        Ok(Some(_)) => not_found(board),
        Ok(None) => projects::not_found(project),
        Err(error) => error,
    }
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
    tickets: Arc<dyn TicketRepository>,
    runners: Arc<RunnerService>,
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
        tickets: Arc<dyn TicketRepository>,
        runners: Arc<RunnerService>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            repository,
            projects,
            tickets,
            runners,
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

    /// Points the board to the repository checked out at `path` on `runner`.
    pub async fn set_repository(
        &self,
        project: Id<Project>,
        id: Id<Board>,
        runner: Id<Runner>,
        path: &str,
        expected: Version,
    ) -> Result<BoardSummary, AppError> {
        let current = self.get(project, id).await?;
        if current.board.version() != expected {
            return Err(AppError::version_conflict("board", id));
        }
        let repository = self.runners.checkout(runner, path).await?;
        let name = repository.name().as_str().to_owned();
        self.point(current, Some(repository.id()), Some(name), expected)
            .await
    }

    /// The board without a repository; one without returns unchanged.
    pub async fn clear_repository(
        &self,
        project: Id<Project>,
        id: Id<Board>,
        expected: Version,
    ) -> Result<BoardSummary, AppError> {
        let current = self.get(project, id).await?;
        if current.board.version() != expected {
            return Err(AppError::version_conflict("board", id));
        }
        self.point(current, None, None, expected).await
    }

    async fn point(
        &self,
        current: BoardSummary,
        repository: Option<Id<Repository>>,
        repository_name: Option<String>,
        expected: Version,
    ) -> Result<BoardSummary, AppError> {
        let Some(next) = current.board.with_repository(repository, self.clock.now()) else {
            return Ok(current);
        };
        let change = self.repository.update(&next, expected).await?;
        self.signal.notify(&change);
        Ok(BoardSummary {
            board: next,
            repository_name,
            ..current
        })
    }

    /// Boards pointing to the repository `path` on `runner` is in; none outside of one.
    pub async fn list_by_checkout(
        &self,
        runner: Id<Runner>,
        path: &str,
    ) -> Result<Vec<BoardSummary>, AppError> {
        match self.runners.key(runner, path).await? {
            Some(key) => self.repository.list_by_checkout(runner, &key).await,
            None => Ok(Vec::new()),
        }
    }

    /// Deletes the board with its tickets; open ones only with `delete_open`, as in legacy.
    pub async fn delete(
        &self,
        project: Id<Project>,
        id: Id<Board>,
        expected: Version,
        delete_open: bool,
        actor: &Actor,
        from_agent: bool,
    ) -> Result<Vec<TicketKey>, AppError> {
        if from_agent {
            return Err(crate::auth::decision_needs_person("deleting a board"));
        }
        let mut write = self.tickets.begin().await?;
        let board = match write.board(id).await? {
            Some((board, _)) if board.project() == project => board,
            _ => {
                drop(write);
                return Err(missing(self.projects.as_ref(), project, id).await);
            }
        };
        if board.version() != expected {
            return Err(AppError::version_conflict("board", id));
        }
        let doomed = write.board_tickets(id).await?;
        let open: Vec<&TicketKey> = doomed
            .iter()
            .filter(|ticket| ticket.status().is_open())
            .map(Ticket::key)
            .collect();
        if !open.is_empty() && !delete_open {
            return Err(tickets::board_has_open_tickets(&open));
        }
        let now = self.clock.now();
        let mut changes =
            tickets::remove(write.as_mut(), &doomed, actor, now, self.ids.as_ref()).await?;
        changes.push(write.delete_board(id, expected, now).await?);
        write.commit().await?;
        if let Some(last) = changes.last() {
            self.signal.notify(last);
        }
        Ok(doomed.iter().map(|ticket| ticket.key().clone()).collect())
    }
}

#[cfg(test)]
mod tests {
    use nullradix_domain::TextError;

    use super::*;

    #[test]
    fn maps_validation_errors_to_stable_codes() {
        assert_eq!(
            invalid(&BoardError::Name(TextError::Empty)).code(),
            "board-name-empty"
        );
        assert_eq!(
            invalid(&BoardError::ActorName(TextError::TooLong(100))).code(),
            "value-too-long"
        );
        let id = Id::from_uuid(uuid::Uuid::from_u128(1));
        assert_eq!(not_found(id).code(), "board-not-found");
    }
}
