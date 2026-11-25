//! Starting a ticket in its own worktree or in one it joins, as legacy `board start` did.

use std::sync::Arc;

use nullradix_domain::Id;
use nullradix_domain::{
    Actor, JoinCandidate, Repository, Runner, StatusFacts, StatusRequest, TargetBranch, Ticket,
    TicketKey, TicketStatus, Worktree, WorktreeDraft, WorktreePurpose, check_status_change,
    pick_join, ticket_branch, ticket_worktree_path,
};

use super::{
    Cleaned, NewWorktree, WorktreeEntry, WorktreeRepository, already_running, board_repo_mismatch,
    host_not_started, invalid,
};
use crate::boards::{self, BoardRepository};
use crate::repositories::RunnerService;
use crate::tickets::{self, TicketService, TicketView, TicketWrite, locked};
use crate::{AppError, ChangeSignal, Clock, IdGenerator};

/// Note shown when git was told to allow long paths in the repository.
const LONG_PATHS_NOTICE: &str =
    "core.longpaths switched on in the repository, so long paths work in worktrees";

/// Where the ticket goes: its own new worktree or one that already exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartMode {
    New,
    /// Joins the worktree of this ticket, or falls back to a new one with a warning.
    JoinTicket(String),
    /// Joins the first open worktree with the same target, else a new one.
    AutoJoin,
}

impl StartMode {
    /// The mode of the two start options, which exclude each other.
    pub fn of(join_ticket: Option<String>, join: bool) -> Result<Self, AppError> {
        match (join_ticket, join) {
            (Some(_), true) => Err(AppError::invalid(
                "input-invalid",
                "join a given ticket or join automatically, not both",
            )),
            (Some(host), false) => Ok(Self::JoinTicket(host)),
            (None, true) => Ok(Self::AutoJoin),
            (None, false) => Ok(Self::New),
        }
    }
}

/// A target branch the start created because it was missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetCreated {
    pub branch: String,
    /// Such as `origin/release`, or the branch or commit of the main checkout.
    pub from: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub ticket: TicketView,
    pub branch: String,
    pub worktree: String,
    /// The ticket joined an existing worktree.
    pub joined: bool,
    pub warnings: Vec<String>,
    pub target_created: Option<TargetCreated>,
    /// Ready worktrees the start removed or kept before it began.
    pub cleanup: Vec<Cleaned>,
}

/// The checkout a start or done happens in.
pub(super) struct Place {
    pub(super) runner: Id<Runner>,
    pub(super) repository: Id<Repository>,
    pub(super) root: String,
    pub(super) cwd: String,
}

enum Link {
    Join(WorktreeEntry),
    New { warning: Option<String> },
}

pub struct WorktreeService {
    pub(super) tickets: Arc<TicketService>,
    boards: Arc<dyn BoardRepository>,
    pub(super) worktrees: Arc<dyn WorktreeRepository>,
    pub(super) runners: Arc<RunnerService>,
    pub(super) clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    pub(super) signal: ChangeSignal,
}

impl std::fmt::Debug for WorktreeService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorktreeService").finish_non_exhaustive()
    }
}

/// The in-progress locks; a ticket already in progress passes.
fn check_start(ticket: &Ticket, open: Vec<TicketKey>, force: bool) -> Result<(), AppError> {
    let facts = StatusFacts {
        acceptance_blank: false,
        open_dependencies: open,
        open_questions: Vec::new(),
    };
    let request = StatusRequest {
        by_person: false,
        force,
    };
    check_status_change(
        Some(ticket.status()),
        TicketStatus::InProgress,
        request,
        &facts,
    )
    .map_err(|error| locked(&error))
}

fn refuse_started(ticket: &Ticket) -> Result<(), AppError> {
    match &ticket.record().branch {
        Some(branch) => Err(already_running(ticket.key(), branch)),
        None => Ok(()),
    }
}

impl WorktreeService {
    #[must_use]
    pub fn new(
        tickets: Arc<TicketService>,
        boards: Arc<dyn BoardRepository>,
        worktrees: Arc<dyn WorktreeRepository>,
        runners: Arc<RunnerService>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            tickets,
            boards,
            worktrees,
            runners,
            clock,
            ids,
            signal,
        }
    }

    /// Starts the ticket from `cwd` on `runner`, checking in the legacy order.
    pub async fn start(
        &self,
        key: &str,
        runner: Id<Runner>,
        cwd: &str,
        mode: StartMode,
        force: bool,
        actor: &Actor,
    ) -> Result<Started, AppError> {
        let view = self.tickets.get(key).await?.view;
        let ticket = view.ticket;
        let place = self.place(&ticket, runner, cwd).await?;
        refuse_started(&ticket)?;
        check_start(&ticket, view.depends_open, force)?;
        let cleanup = self.cleanup(&place).await?;
        let link = match mode {
            StartMode::New => Link::New { warning: None },
            StartMode::JoinTicket(host) => self.host(&ticket, &host, &place).await?,
            StartMode::AutoJoin => match self.find_join(&ticket, &place).await? {
                Some(entry) => Link::Join(entry),
                None => Link::New { warning: None },
            },
        };
        let started = match link {
            Link::Join(entry) => self.join(&ticket, entry, force, actor).await?,
            Link::New { warning } => self.create(&ticket, &place, warning, force, actor).await?,
        };
        Ok(Started { cleanup, ..started })
    }

    /// The checkout of `cwd`, which must be the repository of the ticket's board.
    pub(super) async fn place(
        &self,
        ticket: &Ticket,
        runner: Id<Runner>,
        cwd: &str,
    ) -> Result<Place, AppError> {
        let board = ticket.record().board;
        let summary = self
            .boards
            .get(board)
            .await?
            .ok_or_else(|| boards::not_found(board))?;
        let (located, found) = self.runners.recorded(runner, cwd).await?;
        match (summary.board.repository(), found) {
            (Some(wanted), Some(repository)) if repository.id() == wanted => Ok(Place {
                runner,
                repository: wanted,
                root: located.root,
                cwd: cwd.trim().to_owned(),
            }),
            _ => Err(board_repo_mismatch(ticket.key())),
        }
    }

    /// The host's worktree, or a new one with a warning when the targets differ.
    async fn host(&self, ticket: &Ticket, host: &str, place: &Place) -> Result<Link, AppError> {
        let host = self.tickets.get(host).await?.view.ticket;
        let entry = self
            .worktrees
            .of_ticket(host.id())
            .await?
            .filter(|entry| {
                let record = entry.worktree.record();
                record.repository == place.repository && record.runner == place.runner
            })
            .ok_or_else(|| host_not_started(host.key()))?;
        if ticket.record().target_branch != host.record().target_branch {
            return Ok(Link::New {
                warning: Some(format!(
                    "{} has another target branch than {}; it gets its own worktree",
                    ticket.key(),
                    host.key()
                )),
            });
        }
        // touches are not ported yet, so no two tickets overlap
        Ok(Link::Join(entry))
    }

    /// The first open work worktree in registry order the ticket may join.
    async fn find_join(
        &self,
        ticket: &Ticket,
        place: &Place,
    ) -> Result<Option<WorktreeEntry>, AppError> {
        let mut entries: Vec<WorktreeEntry> = self
            .worktrees
            .list(place.repository, place.runner)
            .await?
            .into_iter()
            .filter(|entry| entry.worktree.record().purpose == WorktreePurpose::Work)
            .collect();
        let paths: Vec<String> = entries
            .iter()
            .map(|entry| entry.worktree.path().as_str().to_owned())
            .collect();
        let exists = self.runners.folders_exist(place.runner, &paths).await?;
        let mut candidates = Vec::with_capacity(entries.len());
        for (entry, folder_exists) in entries.iter().zip(exists) {
            let worktree = &entry.worktree;
            let open = !worktree.removed() && !worktree.record().ready_to_merge && folder_exists;
            candidates.push(JoinCandidate {
                worktree,
                folder_exists,
                targets: if open {
                    self.targets(entry).await?
                } else {
                    Vec::new()
                },
                overlap: false,
            });
        }
        let picked = pick_join(&candidates, ticket.record().target_branch.as_ref());
        drop(candidates);
        Ok(picked.map(|index| entries.swap_remove(index)))
    }

    /// Targets of the entry's tickets; deleted ones drop out as in legacy.
    async fn targets(&self, entry: &WorktreeEntry) -> Result<Vec<Option<TargetBranch>>, AppError> {
        let mut targets = Vec::with_capacity(entry.tickets.len());
        for link in &entry.tickets {
            if let Some(read) = self.tickets.repository().get(&link.key).await? {
                targets.push(read.row.ticket.record().target_branch.clone());
            }
        }
        Ok(targets)
    }

    async fn join(
        &self,
        ticket: &Ticket,
        entry: WorktreeEntry,
        force: bool,
        actor: &Actor,
    ) -> Result<Started, AppError> {
        let mut write = self.tickets.repository().begin().await?;
        let before = fresh(write.as_mut(), ticket.key(), force).await?;
        write
            .join_worktree(entry.worktree.id(), before.id(), self.clock.now())
            .await?;
        self.link(write, &before, &entry.worktree, actor, "joined")
            .await?;
        self.started(ticket.key(), &entry.worktree, true, Vec::new(), None)
            .await
    }

    /// The git step first; the records follow in one write, without rollback as in legacy.
    async fn create(
        &self,
        ticket: &Ticket,
        place: &Place,
        warning: Option<String>,
        force: bool,
        actor: &Actor,
    ) -> Result<Started, AppError> {
        let record = ticket.record();
        let branch = ticket_branch(&record.key, record.title.as_str());
        let path =
            ticket_worktree_path(&place.root, &record.key).map_err(|error| invalid(&error))?;
        let target = record
            .target_branch
            .as_ref()
            .map(|name| name.as_str().to_owned());
        let request = NewWorktree {
            cwd: place.cwd.clone(),
            root: place.root.clone(),
            branch: branch.as_str().to_owned(),
            path: path.as_str().to_owned(),
            target: target.clone(),
        };
        // a start committing after this check still leaves git's work behind, as in legacy
        if let Some(entry) = self.worktrees.of_ticket(ticket.id()).await? {
            return Err(already_running(
                ticket.key(),
                entry.worktree.branch().as_str(),
            ));
        }
        let created = self.runners.create_worktree(place.runner, &request).await?;
        let now = self.clock.now();
        let draft = WorktreeDraft {
            repository: place.repository,
            runner: place.runner,
            branch,
            target_branch: record.target_branch.clone(),
            path,
            purpose: WorktreePurpose::Work,
        };
        let worktree = Worktree::new(self.ids.next(), draft, now).seen_present(now);
        let mut write = self.tickets.repository().begin().await?;
        let before = fresh(write.as_mut(), ticket.key(), force).await?;
        // git proved branch and folder gone, so a live row with this branch is stale
        write
            .retire_branch(place.repository, place.runner, worktree.branch(), now)
            .await?;
        write.insert_worktree(&worktree, before.id()).await?;
        self.link(write, &before, &worktree, actor, "started")
            .await?;
        let mut warnings: Vec<String> = warning.into_iter().collect();
        if created.long_paths_enabled {
            warnings.push(LONG_PATHS_NOTICE.to_owned());
        }
        let target_created = target
            .zip(created.target_created_from)
            .map(|(branch, from)| TargetCreated { branch, from });
        self.started(ticket.key(), &worktree, false, warnings, target_created)
            .await
    }

    /// Shows the worktree on the ticket and moves it to in progress, then commits.
    async fn link(
        &self,
        mut write: Box<dyn TicketWrite>,
        before: &Ticket,
        worktree: &Worktree,
        actor: &Actor,
        note: &str,
    ) -> Result<(), AppError> {
        let record = before.record();
        let mut after = record.clone();
        after.branch = Some(worktree.branch().as_str().to_owned());
        after.worktree = Some(worktree.path().as_str().to_owned());
        after.status = TicketStatus::InProgress;
        after.blocked_reason = None;
        if record.status != TicketStatus::InProgress {
            after.sort_order = write
                .next_sort_order(record.board, TicketStatus::InProgress)
                .await?;
        }
        let now = self.clock.now();
        let revised = before.revised(after, now);
        let history = self
            .tickets
            .history(before, &revised, actor, now, Some(note));
        let change = write.update(&revised, record.version, &history).await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(())
    }

    async fn started(
        &self,
        key: &TicketKey,
        worktree: &Worktree,
        joined: bool,
        warnings: Vec<String>,
        target_created: Option<TargetCreated>,
    ) -> Result<Started, AppError> {
        Ok(Started {
            ticket: self.tickets.get(&key.to_string()).await?.view,
            branch: worktree.branch().as_str().to_owned(),
            worktree: worktree.path().as_str().to_owned(),
            joined,
            warnings,
            target_created,
            cleanup: Vec::new(),
        })
    }
}

/// The ticket read again inside the write, so a start meanwhile is still refused.
async fn fresh(
    write: &mut dyn TicketWrite,
    key: &TicketKey,
    force: bool,
) -> Result<Ticket, AppError> {
    let ticket = tickets::load(write, key, None).await?;
    refuse_started(&ticket)?;
    let index = write.dependency_index().await?;
    check_start(
        &ticket,
        index.open_dependencies(&ticket.record().depends_on),
        force,
    )?;
    Ok(ticket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_join_options_exclude_each_other() {
        assert_eq!(StartMode::of(None, false).unwrap(), StartMode::New);
        assert_eq!(StartMode::of(None, true).unwrap(), StartMode::AutoJoin);
        assert_eq!(
            StartMode::of(Some(String::from("ARC-1")), false).unwrap(),
            StartMode::JoinTicket(String::from("ARC-1"))
        );
        let both = StartMode::of(Some(String::from("ARC-1")), true).unwrap_err();
        assert_eq!(both.code(), "input-invalid");
    }
}
