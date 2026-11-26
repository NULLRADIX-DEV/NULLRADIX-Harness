//! Where a worktree stands with its setup commands, which run before Claude starts in it.

use nullradix_domain::{Id, SetupState, Worktree};

use super::WorktreeService;
use crate::AppError;

impl WorktreeService {
    /// The live worktree of the ticket and its setup state; none when the ticket has none.
    pub async fn setup_state(
        &self,
        key: &str,
    ) -> Result<Option<(Id<Worktree>, SetupState)>, AppError> {
        let ticket = self.tickets.get(key).await?.view.ticket;
        Ok(self
            .worktrees
            .of_ticket(ticket.id())
            .await?
            .map(|entry| (entry.worktree.id(), entry.worktree.record().setup)))
    }

    /// Records the setup state of a worktree.
    pub async fn mark_setup(
        &self,
        worktree: Id<Worktree>,
        state: SetupState,
    ) -> Result<(), AppError> {
        if let Some(change) = self
            .worktrees
            .set_setup(worktree, state, self.clock.now())
            .await?
        {
            self.signal.notify(&change);
        }
        Ok(())
    }
}
