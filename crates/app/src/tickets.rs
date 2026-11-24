//! Tickets on boards: creating them under the legacy locks and reading them in display order.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Actor, Board, Column, DependencyIndex, HistoryEntry, Id, Priority, Project, ProjectKey,
    RuleError, StatusFacts, StatusLock, StatusRequest, TargetBranch, Ticket, TicketDraft,
    TicketError, TicketKey, TicketStatus, TicketTitle, Timestamp, check_status_change,
    display_order, normalize_dependencies, normalize_labels, validate_dependencies,
};

use crate::projects::{self, ProjectRepository};
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator, boards};

/// A ticket with the name of the actor who created it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRow {
    pub ticket: Ticket,
    pub created_by_name: String,
}

/// One line of a ticket's history as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryLine {
    pub at: Timestamp,
    pub actor: Id<Actor>,
    pub actor_name: String,
    pub field: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub note: Option<String>,
}

/// Which tickets a list reads; both narrow it, none reads every board.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TicketScope {
    pub project: Option<Id<Project>>,
    pub board: Option<Id<Board>>,
}

#[async_trait]
pub trait TicketRepository: Send + Sync {
    /// Starts a write that reads and changes tickets in one transaction.
    async fn begin(&self) -> Result<Box<dyn TicketWrite>, AppError>;
    /// Fails with a not-found error when the scope names a missing project or board.
    async fn list(&self, scope: TicketScope) -> Result<Vec<TicketRow>, AppError>;
    async fn get(&self, key: &TicketKey) -> Result<Option<TicketRow>, AppError>;
    async fn dependency_index(&self) -> Result<DependencyIndex, AppError>;
    async fn history(&self, ticket: Id<Ticket>) -> Result<Vec<HistoryLine>, AppError>;
}

/// One write; dropping it without [`TicketWrite::commit`] undoes everything.
#[async_trait]
pub trait TicketWrite: Send {
    /// The board with the key of its project.
    async fn board(&mut self, id: Id<Board>) -> Result<Option<(Board, ProjectKey)>, AppError>;
    /// Tickets on the board that are neither done nor not needed.
    async fn open_titles(
        &mut self,
        board: Id<Board>,
    ) -> Result<Vec<(TicketKey, TicketTitle)>, AppError>;
    async fn dependency_index(&mut self) -> Result<DependencyIndex, AppError>;
    /// The next number of the project; numbers are never handed out twice.
    async fn take_number(&mut self, project: Id<Project>) -> Result<u32, AppError>;
    /// The place after the last ticket in this column of the board.
    async fn next_sort_order(
        &mut self,
        board: Id<Board>,
        status: TicketStatus,
    ) -> Result<i64, AppError>;
    async fn insert(
        &mut self,
        ticket: &Ticket,
        history: &[HistoryEntry],
    ) -> Result<Change, AppError>;
    async fn commit(self: Box<Self>) -> Result<(), AppError>;
}

/// A ticket as boards show it: its column and how it relates to other tickets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketView {
    pub ticket: Ticket,
    pub created_by_name: String,
    pub column: Column,
    pub depends_open: Vec<TicketKey>,
    pub dependents: Vec<TicketKey>,
    /// Dependents that become ready once this ticket is done.
    pub unlocks: Vec<TicketKey>,
}

impl TicketView {
    fn new(row: TicketRow, index: &DependencyIndex) -> Self {
        let record = row.ticket.record();
        let depends_open = index.open_dependencies(&record.depends_on);
        Self {
            column: Column::of(record.status, !depends_open.is_empty()),
            dependents: index.dependents(&record.key).to_vec(),
            unlocks: index.unlocks(&record.key),
            depends_open,
            created_by_name: row.created_by_name,
            ticket: row.ticket,
        }
    }

    fn order(&self) -> (Column, u8, u32, String) {
        let record = self.ticket.record();
        display_order(self.column, record.priority, &record.key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketDetail {
    pub view: TicketView,
    pub history: Vec<HistoryLine>,
}

/// Input of a new ticket; what is left out gets the legacy default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewTicket {
    pub title: String,
    pub description: Option<String>,
    pub acceptance: Option<String>,
    pub status: Option<TicketStatus>,
    pub priority: Option<Priority>,
    pub labels: Vec<String>,
    /// Ticket keys as typed, such as `arc-7`.
    pub depends_on: Vec<String>,
    pub needs_planning: Option<bool>,
    pub target_branch: Option<String>,
}

/// Who writes and which legacy locks the write lifts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteOptions {
    /// A person in the board, rather than an agent through the CLI.
    pub by_person: bool,
    /// Lifts the dependency, question and not-needed locks, never the acceptance lock.
    pub force: bool,
    pub allow_duplicate: bool,
}

/// Filters of a ticket list, applied after the scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TicketFilter {
    pub scope: TicketScope,
    pub columns: Option<Vec<Column>>,
    pub needs_planning: Option<bool>,
    /// Matches the assignee exactly.
    pub assignee: Option<String>,
    /// Matches one label exactly.
    pub label: Option<String>,
    /// Case-insensitive text in the key, title or description.
    pub query: Option<String>,
}

impl TicketFilter {
    fn accepts(&self, view: &TicketView, query: Option<&str>) -> bool {
        let record = view.ticket.record();
        self.columns
            .as_ref()
            .is_none_or(|columns| columns.contains(&view.column))
            && self
                .needs_planning
                .is_none_or(|wanted| record.needs_planning == wanted)
            && self
                .assignee
                .as_ref()
                .is_none_or(|wanted| record.assignee.as_ref() == Some(wanted))
            && self
                .label
                .as_ref()
                .is_none_or(|wanted| record.labels.contains(wanted))
            && query.is_none_or(|query| {
                format!(
                    "{} {} {}",
                    record.key,
                    record.title.as_str(),
                    record.description
                )
                .to_lowercase()
                .contains(query)
            })
    }
}

#[must_use]
pub fn invalid(error: &TicketError) -> AppError {
    let message = error.to_string();
    match error {
        TicketError::InvalidKey(_) => AppError::invalid("ticket-id-invalid", message),
        TicketError::InvalidStatus(_) | TicketError::WaitingIsDerived => {
            AppError::invalid("status-invalid", message)
        }
        TicketError::InvalidPriority(_) => AppError::invalid("priority-invalid", message),
        TicketError::Title(text) => AppError::text(*text, "title-empty", message),
        TicketError::TargetBranchTooLong(_) => AppError::invalid("value-too-long", message),
        TicketError::InvalidTargetBranch(_) | TicketError::TicketBranchTarget(_) => {
            AppError::invalid("target-branch-invalid", message)
        }
    }
}

fn broken_rule(error: &RuleError) -> AppError {
    let code = match error {
        RuleError::LabelTooLong { .. } => "value-too-long",
        RuleError::SelfDependency(_) => "dependency-self",
        RuleError::UnknownDependency(_) => "dependency-unknown",
        RuleError::DependencyCycle(_) => "dependency-cycle",
    };
    AppError::invalid(code, error.to_string())
}

fn locked(error: &StatusLock) -> AppError {
    let code = match error {
        StatusLock::NotNeededByPeople => "not-needed-human-only",
        StatusLock::AcceptanceMissing => "acceptance-missing",
        StatusLock::DependencyOpen(_) => "dependency-open",
        StatusLock::QuestionOpen(_) => "question-open",
    };
    AppError::invalid(code, error.to_string())
}

#[must_use]
pub fn not_found(key: &TicketKey) -> AppError {
    AppError::not_found("ticket-not-found", format!("ticket {key} does not exist"))
}

fn duplicate(existing: &TicketKey) -> AppError {
    AppError::conflict(
        "ticket-duplicate",
        format!("the open ticket {existing} already has this title"),
    )
    .with_existing(existing.to_string())
}

/// Text input checked before the write starts, so a refused ticket takes no number.
struct CheckedInput {
    title: TicketTitle,
    labels: Vec<String>,
    depends_on: Vec<TicketKey>,
    target_branch: Option<TargetBranch>,
}

impl CheckedInput {
    fn new(input: &NewTicket) -> Result<Self, AppError> {
        let title = TicketTitle::parse(&input.title).map_err(|error| invalid(&error))?;
        let labels = normalize_labels(&input.labels).map_err(|error| broken_rule(&error))?;
        let depends_on = input
            .depends_on
            .iter()
            .map(|key| TicketKey::parse(key))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| invalid(&error))?;
        let target_branch = match input.target_branch.as_deref() {
            Some(text) => TargetBranch::parse(text).map_err(|error| invalid(&error))?,
            None => None,
        };
        Ok(Self {
            title,
            labels,
            depends_on: normalize_dependencies(depends_on),
            target_branch,
        })
    }
}

pub struct TicketService {
    repository: Arc<dyn TicketRepository>,
    projects: Arc<dyn ProjectRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for TicketService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TicketService").finish_non_exhaustive()
    }
}

impl TicketService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn TicketRepository>,
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

    /// Creates a ticket at the end of its column, checked in the order legacy checked it.
    pub async fn create(
        &self,
        project: Id<Project>,
        board: Id<Board>,
        input: &NewTicket,
        actor: &Actor,
        options: WriteOptions,
    ) -> Result<TicketView, AppError> {
        let checked = CheckedInput::new(input)?;
        let mut write = self.repository.begin().await?;
        let (board, project_key) = match write.board(board).await? {
            Some((found, key)) if found.project() == project => (found, key),
            _ => {
                drop(write);
                return Err(self.missing_board(project, board).await);
            }
        };
        if !options.allow_duplicate {
            let wanted = checked.title.folded();
            if let Some((existing, _)) = write
                .open_titles(board.id())
                .await?
                .into_iter()
                .find(|(_, title)| title.folded() == wanted)
            {
                return Err(duplicate(&existing));
            }
        }
        let key = TicketKey::new(project_key, write.take_number(project).await?);
        let index = write.dependency_index().await?;
        if !checked.depends_on.is_empty() {
            validate_dependencies(&key, &checked.depends_on, index.edges())
                .map_err(|error| broken_rule(&error))?;
        }
        let acceptance = input.acceptance.clone().unwrap_or_default();
        let status = input.status.unwrap_or(TicketStatus::Backlog);
        let facts = StatusFacts {
            acceptance_blank: acceptance.trim().is_empty(),
            open_dependencies: index.open_dependencies(&checked.depends_on),
            open_questions: Vec::new(),
        };
        let request = StatusRequest {
            by_person: options.by_person,
            force: options.force,
        };
        check_status_change(None, status, request, &facts).map_err(|error| locked(&error))?;
        let sort_order = write.next_sort_order(board.id(), status).await?;
        let draft = TicketDraft {
            title: checked.title,
            description: input.description.clone().unwrap_or_default(),
            acceptance,
            status,
            priority: input.priority.unwrap_or_default(),
            labels: checked.labels,
            depends_on: checked.depends_on,
            needs_planning: input.needs_planning.unwrap_or(true),
            target_branch: checked.target_branch,
        };
        let ticket = Ticket::new(
            self.ids.next(),
            key,
            &board,
            draft,
            sort_order,
            actor.id(),
            self.clock.now(),
        );
        let history = [HistoryEntry::created(self.ids.next(), &ticket)];
        let change = write.insert(&ticket, &history).await?;
        write.commit().await?;
        self.signal.notify(&change);
        let row = TicketRow {
            ticket,
            created_by_name: actor.name().as_str().to_owned(),
        };
        Ok(TicketView::new(row, &index))
    }

    /// Tickets in display order: column, priority, number.
    pub async fn list(&self, filter: &TicketFilter) -> Result<Vec<TicketView>, AppError> {
        let rows = self.repository.list(filter.scope).await?;
        let index = self.repository.dependency_index().await?;
        let query = filter.query.as_ref().map(|text| text.to_lowercase());
        let mut views: Vec<TicketView> = rows
            .into_iter()
            .map(|row| TicketView::new(row, &index))
            .filter(|view| filter.accepts(view, query.as_deref()))
            .collect();
        views.sort_by_cached_key(TicketView::order);
        Ok(views)
    }

    /// The ticket with its history; the key may be typed loosely, such as `arc-7`.
    pub async fn get(&self, key: &str) -> Result<TicketDetail, AppError> {
        let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
        let row = self
            .repository
            .get(&key)
            .await?
            .ok_or_else(|| not_found(&key))?;
        let index = self.repository.dependency_index().await?;
        let history = self.repository.history(row.ticket.id()).await?;
        Ok(TicketDetail {
            view: TicketView::new(row, &index),
            history,
        })
    }

    async fn missing_board(&self, project: Id<Project>, board: Id<Board>) -> AppError {
        match self.projects.get(project).await {
            Ok(Some(_)) => boards::not_found(board),
            Ok(None) => projects::not_found(project),
            Err(error) => error,
        }
    }
}

#[cfg(test)]
mod tests {
    use nullradix_domain::TextError;

    use super::*;

    #[test]
    fn maps_ticket_errors_to_legacy_codes() {
        let key = TicketKey::parse("ARC-1").unwrap();
        let cases = [
            (
                invalid(&TicketError::InvalidKey(String::from("x"))),
                "ticket-id-invalid",
            ),
            (
                invalid(&TicketError::Title(TextError::Empty)),
                "title-empty",
            ),
            (
                invalid(&TicketError::Title(TextError::TooLong(400))),
                "value-too-long",
            ),
            (
                invalid(&TicketError::TicketBranchTarget(String::from("ticket"))),
                "target-branch-invalid",
            ),
            (
                broken_rule(&RuleError::DependencyCycle(vec![key.clone()])),
                "dependency-cycle",
            ),
            (
                locked(&StatusLock::NotNeededByPeople),
                "not-needed-human-only",
            ),
            (not_found(&key), "ticket-not-found"),
            (duplicate(&key), "ticket-duplicate"),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code);
        }
        assert_eq!(duplicate(&key).existing(), Some("ARC-0001"));
    }
}
