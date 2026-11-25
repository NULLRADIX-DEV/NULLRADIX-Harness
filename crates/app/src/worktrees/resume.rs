//! Bringing a started ticket's worktree back before work resumes, as legacy `resume` did.

use nullradix_domain::{Actor, ActualState, DesiredState, Ticket};

use super::{
    EnsureWorktree, EnsuredWorktree, WorktreeEntry, WorktreeService, git_failed, ticket_not_started,
};
use crate::AppError;
use crate::repositories::not_a_git_project;
use crate::tickets::{TicketPatch, WriteOptions};

/// A worktree that is on its branch again, or was never gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumedWorktree {
    pub branch: String,
    pub path: String,
    /// The folder was added again.
    pub restored: bool,
    /// A leftover folder that was in the way and now lives here.
    pub moved_aside: Option<String>,
}

/// What a resume did with the ticket's worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resumed {
    Ready(ResumedWorktree),
    /// The worktree was cleaned up: branch and worktree are cleared, the old branch stays.
    Reset(String),
    /// The branch is not in the repository on the runner, so the ticket waits.
    Skipped(String),
}

impl WorktreeService {
    /// Legacy `ensure_worktree`: the ticket's folder on its branch, added again when it is gone.
    pub async fn ensure(&self, key: &str) -> Result<ResumedWorktree, AppError> {
        let (_, entry) = self.linked(key).await?;
        self.restore(&entry).await?.ok_or_else(|| missing(&entry))
    }

    /// Legacy `resume`: a cleaned-up worktree is not restored, the ticket starts afresh.
    pub async fn resume(&self, key: &str, actor: &Actor) -> Result<Resumed, AppError> {
        let (ticket, entry) = self.linked(key).await?;
        let record = entry.worktree.record();
        if entry.worktree.removed() {
            let patch = TicketPatch {
                clear_link: true,
                ..TicketPatch::default()
            };
            let options = WriteOptions {
                force: true,
                ..WriteOptions::default()
            };
            let version = ticket.record().version;
            self.tickets
                .update(key, &patch, actor, version, options)
                .await?;
            return Ok(Resumed::Reset(format!(
                "the worktree {} (branch {}) was cleaned up and is not restored; the ticket \
                 starts on its own new branch, the old branch stays in the repository",
                record.path.as_str(),
                record.branch.as_str()
            )));
        }
        Ok(match self.restore(&entry).await? {
            Some(worktree) => Resumed::Ready(worktree),
            None => Resumed::Skipped(format!(
                "branch {} does not exist on this machine",
                record.branch.as_str()
            )),
        })
    }

    /// The ticket and the worktree it is linked to; a ticket without both is not started.
    async fn linked(&self, key: &str) -> Result<(Ticket, WorktreeEntry), AppError> {
        let ticket = self.tickets.get(key).await?.view.ticket;
        let entry = self.worktrees.of_ticket(ticket.id()).await?;
        match entry {
            Some(entry) if ticket.record().branch.is_some() => Ok((ticket, entry)),
            _ => Err(ticket_not_started(ticket.key())),
        }
    }

    /// Asks the runner for the folder and records it as present; `None` without the branch.
    async fn restore(&self, entry: &WorktreeEntry) -> Result<Option<ResumedWorktree>, AppError> {
        let worktree = &entry.worktree;
        let record = worktree.record();
        let root = self
            .runners
            .root(record.runner, record.repository)
            .await?
            .ok_or_else(|| not_a_git_project(record.path.as_str()))?;
        let request = EnsureWorktree {
            root,
            branch: record.branch.as_str().to_owned(),
            path: record.path.as_str().to_owned(),
        };
        let ensured = self
            .runners
            .ensure_worktree(record.runner, &request)
            .await?;
        let (restored, moved_aside) = match ensured {
            EnsuredWorktree::BranchMissing => return Ok(None),
            EnsuredWorktree::Present => (false, None),
            EnsuredWorktree::Restored { moved_aside } => (true, moved_aside),
        };
        if record.desired != DesiredState::Present || record.actual != ActualState::Present {
            let mut write = self.tickets.repository().begin().await?;
            let change = write.mark_restored(worktree.id(), self.clock.now()).await?;
            write.commit().await?;
            if let Some(change) = change {
                self.signal.notify(&change);
            }
        }
        Ok(Some(ResumedWorktree {
            branch: request.branch,
            path: request.path,
            restored,
            moved_aside,
        }))
    }
}

/// The branch is gone, which git would have reported as an invalid reference.
fn missing(entry: &WorktreeEntry) -> AppError {
    let record = entry.worktree.record();
    git_failed(
        &[
            "worktree",
            "add",
            record.path.as_str(),
            record.branch.as_str(),
        ],
        &format!("invalid reference: {}", record.branch.as_str()),
    )
}
