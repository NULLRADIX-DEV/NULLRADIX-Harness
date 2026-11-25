//! Worktree records: legacy `worktrees.json` with the tickets each worktree served.

use async_trait::async_trait;
use nullradix_domain::{
    Id, Repository, Runner, Ticket, Timestamp, Worktree, WorktreeBranch, WorktreeTicket,
};

use crate::{AppError, Change};

/// A worktree with every ticket it ever served, in join order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub worktree: Worktree,
    pub tickets: Vec<WorktreeTicket>,
}

impl WorktreeEntry {
    /// The ticket that started the worktree.
    #[must_use]
    pub fn owner(&self) -> Option<&WorktreeTicket> {
        self.tickets.first()
    }
}

/// Link changes inside a ticket write, so they commit together with the ticket.
#[async_trait]
pub trait WorktreeWrite: Send {
    /// Stores the worktree with `owner` as its first ticket.
    async fn insert_worktree(
        &mut self,
        worktree: &Worktree,
        owner: Id<Ticket>,
    ) -> Result<Change, AppError>;
    /// Adds the ticket at the end of the list, or links it again at its old place.
    async fn join_worktree(
        &mut self,
        worktree: Id<Worktree>,
        ticket: Id<Ticket>,
        at: Timestamp,
    ) -> Result<Change, AppError>;
    /// Ends the ticket's live link; the ticket stays in the list. `None` when it had none.
    async fn release_link(
        &mut self,
        ticket: Id<Ticket>,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    /// The worktree the ticket is linked to right now.
    async fn live_worktree(
        &mut self,
        ticket: Id<Ticket>,
    ) -> Result<Option<WorktreeEntry>, AppError>;
}

/// Reads of the registry; links change only with a ticket, through [`WorktreeWrite`].
#[async_trait]
pub trait WorktreeRepository: Send + Sync {
    async fn get(&self, id: Id<Worktree>) -> Result<Option<WorktreeEntry>, AppError>;
    /// The worktree the ticket is linked to right now.
    async fn of_ticket(&self, ticket: Id<Ticket>) -> Result<Option<WorktreeEntry>, AppError>;
    /// Worktrees of the repository on the runner in registry order, oldest first.
    async fn list(
        &self,
        repository: Id<Repository>,
        runner: Id<Runner>,
    ) -> Result<Vec<WorktreeEntry>, AppError>;
}

/// Another live worktree of the repository on that runner has the branch.
#[must_use]
pub fn branch_exists(branch: &WorktreeBranch) -> AppError {
    AppError::invalid(
        "branch-exists",
        format!("a worktree with branch {} exists already", branch.as_str()),
    )
}

/// The ticket is linked to a worktree already.
#[must_use]
pub fn already_started(ticket: Id<Ticket>) -> AppError {
    AppError::invalid(
        "already-started",
        format!("ticket {ticket} has a branch and worktree already"),
    )
}

#[must_use]
pub fn not_found(id: Id<Worktree>) -> AppError {
    AppError::not_found(
        "worktree-not-found",
        format!("worktree {id} does not exist"),
    )
}

/// A ticket change through the API may clear branch and worktree, never set them.
#[must_use]
pub fn link_only_clears() -> AppError {
    AppError::invalid(
        "input-invalid",
        "branch and worktree can only be cleared (null) here, never set",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    #[test]
    fn link_errors_are_bad_input_as_in_legacy() {
        let branch = WorktreeBranch::parse("ticket/ARC-0001-a").unwrap();
        let ticket = Id::from_uuid(uuid::Uuid::from_u128(1));
        for (error, code) in [
            (branch_exists(&branch), "branch-exists"),
            (already_started(ticket), "already-started"),
            (link_only_clears(), "input-invalid"),
        ] {
            assert_eq!((error.kind(), error.code()), (ErrorKind::Invalid, code));
        }
        let missing = not_found(Id::from_uuid(uuid::Uuid::from_u128(2)));
        assert_eq!(missing.kind(), ErrorKind::NotFound);
    }
}
