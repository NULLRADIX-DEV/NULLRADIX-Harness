//! Finishing a ticket as legacy `board done` and `accept` did: readiness, then cleanup.

use nullradix_domain::{
    Actor, Id, Runner, Ticket, TicketStatus, Version, Worktree, WorktreePurpose, ready_to_merge,
};

use super::start::Place;
use super::{RemoveWorktree, WorktreeRemoval, WorktreeService};
use crate::AppError;
use crate::tickets::{TicketView, WriteOptions};

/// A ready worktree the cleanup removed or kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleaned {
    pub path: String,
    pub branch: String,
    pub removal: WorktreeRemoval,
}

/// A ticket moved to done; nothing was merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub ticket: TicketView,
    /// The ticket's branch, when it runs in a worktree.
    pub branch: Option<String>,
    /// Every ticket still on the branch is finished now.
    pub ready_to_merge: bool,
    pub cleanup: Vec<Cleaned>,
}

/// Input of a done besides the ticket and the place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoneRequest<'a> {
    pub acceptance: Option<&'a str>,
    pub expected: Option<Version>,
    pub options: WriteOptions,
}

impl WorktreeService {
    /// Legacy `board done`: done with its locks, then readiness, then cleanup; never merges.
    pub async fn done(
        &self,
        key: &str,
        runner: Id<Runner>,
        cwd: &str,
        request: DoneRequest<'_>,
        actor: &Actor,
    ) -> Result<Finished, AppError> {
        let ticket = self.tickets.get(key).await?.view.ticket;
        let place = self.place(&ticket, runner, cwd).await?;
        let view = self
            .tickets
            .move_to(
                key,
                Some(TicketStatus::Done.as_str()),
                request.acceptance,
                actor,
                request.expected,
                request.options,
            )
            .await?;
        let ready = self.update_ready(ticket.id()).await?;
        let cleanup = self.cleanup(&place).await?;
        Ok(Finished {
            branch: view.ticket.record().branch.clone(),
            ticket: view,
            ready_to_merge: ready.unwrap_or(false),
            cleanup,
        })
    }

    /// Human accept; a ticket with a worktree also updates readiness and cleans up.
    pub async fn accept(
        &self,
        key: &str,
        actor: &Actor,
        expected: Option<Version>,
        by_person: bool,
    ) -> Result<TicketView, AppError> {
        let view = self.tickets.accept(key, actor, expected, by_person).await?;
        let Some(entry) = self.worktrees.of_ticket(view.ticket.id()).await? else {
            return Ok(view);
        };
        self.update_ready(view.ticket.id()).await?;
        let record = entry.worktree.record();
        // legacy cleaned up from the main checkout of the board's repository
        if let Some(root) = self.runners.root(record.runner, record.repository).await? {
            let place = Place {
                runner: record.runner,
                repository: record.repository,
                cwd: root.clone(),
                root,
            };
            self.cleanup(&place).await?;
        }
        Ok(view)
    }

    /// Legacy `update_ready` without `sameSpec`, which needs specs; `None` without a worktree.
    pub(super) async fn update_ready(&self, ticket: Id<Ticket>) -> Result<Option<bool>, AppError> {
        let mut write = self.tickets.repository().begin().await?;
        let Some(entry) = write.live_worktree(ticket).await? else {
            return Ok(None);
        };
        let mut live = Vec::with_capacity(entry.tickets.len());
        for link in entry.tickets.iter().filter(|link| link.live()) {
            if let Some(found) = write.ticket(&link.key).await? {
                live.push(found);
            }
        }
        let ready = ready_to_merge(&live);
        let change = write
            .set_ready(entry.worktree.id(), ready, self.clock.now())
            .await?;
        write.commit().await?;
        if let Some(change) = change {
            self.signal.notify(&change);
        }
        Ok(Some(ready))
    }

    /// Asks the runner to remove each ready work worktree of the place, as the legacy cleaner.
    pub(super) async fn cleanup(&self, place: &Place) -> Result<Vec<Cleaned>, AppError> {
        let entries = self.worktrees.list(place.repository, place.runner).await?;
        let mut cleaned = Vec::new();
        for entry in entries {
            let worktree = &entry.worktree;
            let record = worktree.record();
            if record.purpose != WorktreePurpose::Work
                || !record.ready_to_merge
                || worktree.removed()
            {
                continue;
            }
            // a concurrent cleanup that claimed it first does the work alone
            if !self.claim(worktree.id()).await? {
                continue;
            }
            let request = RemoveWorktree {
                root: place.root.clone(),
                path: worktree.path().as_str().to_owned(),
                cwd: place.cwd.clone(),
            };
            let removal = match self.runners.remove_worktree(place.runner, &request).await {
                Ok(removal) => removal,
                Err(error) => WorktreeRemoval::Kept(error.message().to_owned()),
            };
            self.record_removal(worktree.id(), &removal).await?;
            if removal == WorktreeRemoval::Missing {
                continue;
            }
            cleaned.push(Cleaned {
                path: request.path,
                branch: worktree.branch().as_str().to_owned(),
                removal,
            });
        }
        Ok(cleaned)
    }

    /// Claims the worktree for this cleanup; false when it is gone or claimed already.
    async fn claim(&self, worktree: Id<Worktree>) -> Result<bool, AppError> {
        let mut write = self.tickets.repository().begin().await?;
        let change = write.claim_removal(worktree, self.clock.now()).await?;
        write.commit().await?;
        let claimed = change.is_some();
        if let Some(change) = change {
            self.signal.notify(&change);
        }
        Ok(claimed)
    }

    /// Writes what the runner answered; the row and the branch stay either way.
    async fn record_removal(
        &self,
        worktree: Id<Worktree>,
        removal: &WorktreeRemoval,
    ) -> Result<(), AppError> {
        let now = self.clock.now();
        let mut write = self.tickets.repository().begin().await?;
        let change = match removal {
            WorktreeRemoval::Removed => write.mark_removed(worktree, now).await?,
            WorktreeRemoval::Kept(reason) => write.mark_kept(worktree, Some(reason), now).await?,
            WorktreeRemoval::Missing => write.mark_kept(worktree, None, now).await?,
        };
        write.commit().await?;
        if let Some(change) = change {
            self.signal.notify(&change);
        }
        Ok(())
    }
}
