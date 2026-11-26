//! The planned tickets of every board with the reason, if any, the dispatcher does not take them.

use std::collections::HashMap;
use std::sync::Arc;

use nullradix_domain::{
    Board, DispatcherConfig, Id, NotReady, ReadinessFacts, RepositoryFact, Ticket, TicketKey,
    TicketStatus, not_ready,
};

use crate::AppError;
use crate::boards::BoardRepository;
use crate::repositories::RunnerService;
use crate::tickets::{TicketFilter, TicketService, TicketView};

/// A planned ticket and what keeps the dispatcher from taking it.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub view: TicketView,
    pub board: Id<Board>,
    /// Main checkout of the board's repository on the local runner.
    pub root: Option<String>,
    pub not_ready: Option<NotReady>,
}

impl Candidate {
    #[must_use]
    pub fn key(&self) -> &TicketKey {
        self.view.ticket.key()
    }

    #[must_use]
    pub fn ticket(&self) -> &Ticket {
        &self.view.ticket
    }
}

pub struct Candidates {
    tickets: Arc<TicketService>,
    boards: Arc<dyn BoardRepository>,
    runners: Arc<RunnerService>,
}

impl std::fmt::Debug for Candidates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Candidates").finish_non_exhaustive()
    }
}

impl Candidates {
    #[must_use]
    pub fn new(
        tickets: Arc<TicketService>,
        boards: Arc<dyn BoardRepository>,
        runners: Arc<RunnerService>,
    ) -> Self {
        Self {
            tickets,
            boards,
            runners,
        }
    }

    /// Where a board's repository stands on the local runner, with its main checkout.
    async fn repository(
        &self,
        board: &Board,
    ) -> Result<(RepositoryFact, Option<String>), AppError> {
        let Some(repository) = board.repository() else {
            return Ok((RepositoryFact::Missing, None));
        };
        let local = self.runners.local().id();
        let Some(root) = self.runners.root(local, repository).await? else {
            return Ok((RepositoryFact::NoCheckout, None));
        };
        let exists = self
            .runners
            .folders_exist(local, std::slice::from_ref(&root))
            .await?;
        if exists.first().copied().unwrap_or(false) {
            Ok((RepositoryFact::Present, Some(root)))
        } else {
            Ok((RepositoryFact::FolderGone(root), None))
        }
    }

    /// Planned tickets, urgent first then by number, board after board; `only` narrows to one.
    pub async fn scan(
        &self,
        config: &DispatcherConfig,
        only: Option<Id<Board>>,
    ) -> Result<Vec<Candidate>, AppError> {
        let boards = self.boards.list(None).await?;
        let views = self.tickets.list(&TicketFilter::default()).await?;
        let merged: HashMap<&TicketKey, bool> = views
            .iter()
            .map(|view| {
                let record = view.ticket.record();
                (
                    &record.key,
                    record.status == TicketStatus::Done && record.branch.is_some(),
                )
            })
            .collect();
        let mut found = Vec::new();
        for summary in &boards {
            let board = &summary.board;
            if only.is_some_and(|wanted| wanted != board.id()) {
                continue;
            }
            let (fact, root) = self.repository(board).await?;
            let mut planned: Vec<&TicketView> = views
                .iter()
                .filter(|view| {
                    let record = view.ticket.record();
                    record.board == board.id() && record.status == TicketStatus::Planned
                })
                .collect();
            planned.sort_by_key(|view| {
                let record = view.ticket.record();
                (record.priority.rank(), record.key.number())
            });
            for view in planned {
                let record = view.ticket.record();
                let unmerged: Vec<TicketKey> = record
                    .depends_on
                    .iter()
                    .filter(|key| merged.get(key).copied().unwrap_or(false))
                    .cloned()
                    .collect();
                let questions: Vec<String> = view
                    .open_questions
                    .iter()
                    .map(|question| question.key.to_string())
                    .collect();
                let facts = ReadinessFacts {
                    status: record.status,
                    assignee: record.assignee.as_deref(),
                    needs_planning: record.needs_planning,
                    labels: &record.labels,
                    plan: &record.plan,
                    handbook: &record.handbook,
                    acceptance: &record.acceptance,
                    depends_open: &view.depends_open,
                    depends_unmerged: &unmerged,
                    open_questions: &questions,
                    board_dispatch: board.dispatch_enabled(),
                    repository: fact.clone(),
                };
                found.push(Candidate {
                    not_ready: not_ready(config, &facts),
                    view: view.clone(),
                    board: board.id(),
                    root: root.clone(),
                });
            }
        }
        Ok(found)
    }

    /// The ticket as it is now, with its readiness; none when it is no longer planned.
    pub async fn fresh(
        &self,
        config: &DispatcherConfig,
        key: &TicketKey,
    ) -> Result<Option<Candidate>, AppError> {
        Ok(self
            .scan(config, None)
            .await?
            .into_iter()
            .find(|candidate| candidate.key() == key))
    }
}
