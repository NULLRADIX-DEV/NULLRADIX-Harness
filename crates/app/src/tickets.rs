//! Tickets on boards: creating them under the legacy locks and reading them in display order.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    ASSIGNEE_MAX, Actor, BLOCKED_REASON_MAX, Board, Column, DependencyIndex, HistoryEntry, Id,
    Priority, Project, ProjectKey, RuleError, StatusFacts, StatusLock, StatusRequest, Step,
    StepEdit, StepError, TargetBranch, TextError, Ticket, TicketDraft, TicketError, TicketKey,
    TicketRecord, TicketStatus, TicketTitle, Timestamp, Tombstone, Version, changes,
    check_status_change, display_order, normalize_dependencies, normalize_labels, optional_text,
    step, validate_dependencies,
};

use crate::projects::ProjectRepository;
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator, boards};

/// A ticket with the name of the actor who created it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRow {
    pub ticket: Ticket,
    pub created_by_name: String,
    pub steps: StepProgress,
}

/// How many steps a ticket has and how many of them are done.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepProgress {
    pub done: u32,
    pub total: u32,
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
    /// Reads one snapshot; fails with a not-found error for a missing project or board.
    async fn list(&self, scope: TicketScope) -> Result<TicketSnapshot, AppError>;
    /// Reads the ticket, its history and every dependency in one snapshot.
    async fn get(&self, key: &TicketKey) -> Result<Option<TicketRead>, AppError>;
}

/// Tickets with the dependencies of every ticket, read at the same moment.
#[derive(Debug, Clone, Default)]
pub struct TicketSnapshot {
    pub rows: Vec<TicketRow>,
    pub index: DependencyIndex,
}

#[derive(Debug, Clone)]
pub struct TicketRead {
    pub row: TicketRow,
    pub index: DependencyIndex,
    pub history: Vec<HistoryLine>,
    pub steps: Vec<Step>,
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
    /// The ticket with this key, read for a change.
    async fn ticket(&mut self, key: &TicketKey) -> Result<Option<Ticket>, AppError>;
    /// Tickets on the board by number.
    async fn board_tickets(&mut self, board: Id<Board>) -> Result<Vec<Ticket>, AppError>;
    /// Tickets that depend on `ticket`.
    async fn dependents(&mut self, ticket: Id<Ticket>) -> Result<Vec<Ticket>, AppError>;
    /// Stores the ticket with its labels and dependencies if the row still has `expected`.
    async fn update(
        &mut self,
        ticket: &Ticket,
        expected: Version,
        history: &[HistoryEntry],
    ) -> Result<Change, AppError>;
    async fn delete(&mut self, ticket: &Ticket, tombstone: &Tombstone) -> Result<Change, AppError>;
    /// Deletes the board if it still has version `expected`; its tickets must be gone.
    async fn delete_board(
        &mut self,
        board: Id<Board>,
        expected: Version,
        at: Timestamp,
    ) -> Result<Change, AppError>;
    async fn steps(&mut self, ticket: Id<Ticket>) -> Result<Vec<Step>, AppError>;
    /// Replaces the steps; positions are rewritten from 0.
    async fn replace_steps(&mut self, ticket: Id<Ticket>, steps: &[Step]) -> Result<(), AppError>;
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
    pub steps: StepProgress,
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
            steps: row.steps,
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
    pub steps: Vec<Step>,
}

/// Input of a new ticket; what is left out gets the legacy default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewTicket {
    pub title: String,
    pub description: Option<String>,
    pub acceptance: Option<String>,
    /// A status name such as `planned`; checked like every other field.
    pub status: Option<String>,
    pub priority: Option<String>,
    pub labels: Vec<String>,
    /// Ticket keys as typed, such as `arc-7`; blank entries are skipped.
    pub depends_on: Vec<String>,
    pub needs_planning: Option<bool>,
    pub target_branch: Option<String>,
    /// Texts of work steps, none done.
    pub steps: Vec<String>,
}

/// A change of one step: text, guidance (`Some(None)` removes it) and check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StepPatch {
    pub text: Option<String>,
    pub detail: Option<Option<String>>,
    pub done: Option<bool>,
}

/// What a step change expects to find, as legacy guarded it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StepGuard {
    pub version: Option<Version>,
    /// The text the step at the given number had when the caller read it.
    pub expect_text: Option<String>,
}

/// A change of fields; `None` keeps a field, and `Some(None)` clears one that may be empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TicketPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub plan: Option<String>,
    pub handbook: Option<String>,
    pub diagram: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    /// Replaces the labels.
    pub labels: Option<Vec<String>>,
    pub assignee: Option<Option<String>>,
    pub needs_planning: Option<bool>,
    pub blocked_reason: Option<Option<String>>,
    pub acceptance: Option<String>,
    /// Replaces the dependencies and then ignores the add list; the remove list applies last.
    pub depends_on: Option<Vec<String>>,
    pub depends_on_add: Option<Vec<String>>,
    pub depends_on_remove: Option<Vec<String>>,
    /// Moves the ticket to another board of its project.
    pub board: Option<Id<Board>>,
    pub target_branch: Option<Option<String>>,
}

impl TicketPatch {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    const fn changes_dependencies(&self) -> bool {
        self.depends_on.is_some()
            || self.depends_on_add.is_some()
            || self.depends_on_remove.is_some()
    }
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
        TicketError::Assignee(text) | TicketError::BlockedReason(text) => {
            AppError::text(*text, "value-too-long", message)
        }
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

fn nothing_changed() -> AppError {
    AppError::invalid("nothing-changed", "the change names no field")
}

fn status_missing() -> AppError {
    AppError::invalid("status-missing", "a move needs the status to move to")
}

fn other_project(board: Id<Board>) -> AppError {
    AppError::invalid(
        "board-other-project",
        format!("board {board} belongs to another project; tickets move only within one"),
    )
}

#[must_use]
pub fn board_has_open_tickets(keys: &[&TicketKey]) -> AppError {
    let keys: Vec<String> = keys.iter().map(ToString::to_string).collect();
    AppError::invalid(
        "board-has-open-tickets",
        format!(
            "the board still has open tickets: {}; move them or delete everything",
            keys.join(", ")
        ),
    )
}

fn step_error(error: &StepError) -> AppError {
    let message = error.to_string();
    match error {
        StepError::Text(text) => AppError::text(*text, "step-empty", message),
        StepError::NotFound { .. } | StepError::NoPosition { .. } | StepError::Unknown(_) => {
            AppError::not_found("step-not-found", message)
        }
        StepError::NothingChanged => AppError::invalid("nothing-changed", message),
        StepError::Changed { .. } => AppError::conflict("conflict", message),
    }
}

fn progress(steps: &[Step]) -> StepProgress {
    let count = |steps: usize| u32::try_from(steps).unwrap_or(u32::MAX);
    StepProgress {
        done: count(steps.iter().filter(|step| step.done).count()),
        total: count(steps.len()),
    }
}

fn duplicate(existing: &TicketKey) -> AppError {
    AppError::conflict(
        "ticket-duplicate",
        format!("the open ticket {existing} already has this title"),
    )
    .with_existing(existing.to_string())
}

/// Ticket keys without blank entries and duplicates, in the order given.
fn dependency_keys(texts: &[String]) -> Result<Vec<TicketKey>, AppError> {
    let keys = texts
        .iter()
        .filter(|text| !text.trim().is_empty())
        .map(|text| TicketKey::parse(text))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| invalid(&error))?;
    Ok(normalize_dependencies(keys))
}

/// Columns of a list filter, separated by commas; blank text filters nothing.
pub fn columns(text: &str) -> Result<Option<Vec<Column>>, AppError> {
    let columns = text
        .split(',')
        .filter(|name| !name.trim().is_empty())
        .map(Column::parse)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| invalid(&error))?;
    Ok((!columns.is_empty()).then_some(columns))
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

    /// Creates a ticket at the end of its column, checking in the order legacy checked.
    pub async fn create(
        &self,
        project: Id<Project>,
        board: Id<Board>,
        input: &NewTicket,
        actor: &Actor,
        options: WriteOptions,
    ) -> Result<TicketView, AppError> {
        let status = match input.status.as_deref() {
            Some(text) => TicketStatus::parse(text).map_err(|error| invalid(&error))?,
            None => TicketStatus::Backlog,
        };
        let priority = match input.priority.as_deref() {
            Some(text) => Priority::parse(text).map_err(|error| invalid(&error))?,
            None => Priority::Normal,
        };
        let mut write = self.repository.begin().await?;
        let found = write
            .board(board)
            .await?
            .filter(|(found, _)| found.project() == project);
        if found.is_some() && !options.allow_duplicate {
            // legacy compared the raw title, so a blank one never counts as a duplicate
            let wanted = input.title.trim().to_lowercase();
            if !wanted.is_empty()
                && let Some((existing, _)) = write
                    .open_titles(board)
                    .await?
                    .into_iter()
                    .find(|(_, title)| title.folded() == wanted)
            {
                return Err(duplicate(&existing));
            }
        }
        let Some((board, project_key)) = found else {
            drop(write);
            return Err(boards::missing(self.projects.as_ref(), project, board).await);
        };
        let title = TicketTitle::parse(&input.title).map_err(|error| invalid(&error))?;
        let depends_on = dependency_keys(&input.depends_on)?;
        let steps = step::new_steps(&input.steps, || self.ids.next())
            .map_err(|error| step_error(&error))?;
        let key = TicketKey::new(project_key, write.take_number(project).await?);
        let index = write.dependency_index().await?;
        if !depends_on.is_empty() {
            validate_dependencies(&key, &depends_on, index.edges())
                .map_err(|error| broken_rule(&error))?;
        }
        let labels = normalize_labels(&input.labels).map_err(|error| broken_rule(&error))?;
        let sort_order = write.next_sort_order(board.id(), status).await?;
        let target_branch = match input.target_branch.as_deref() {
            Some(text) => TargetBranch::parse(text).map_err(|error| invalid(&error))?,
            None => None,
        };
        let acceptance = input.acceptance.clone().unwrap_or_default();
        let facts = StatusFacts {
            acceptance_blank: acceptance.trim().is_empty(),
            open_dependencies: index.open_dependencies(&depends_on),
            open_questions: Vec::new(),
        };
        let request = StatusRequest {
            by_person: options.by_person,
            force: options.force,
        };
        check_status_change(None, status, request, &facts).map_err(|error| locked(&error))?;
        let draft = TicketDraft {
            title,
            description: input.description.clone().unwrap_or_default(),
            acceptance,
            status,
            priority,
            labels,
            depends_on,
            needs_planning: input.needs_planning.unwrap_or(true),
            target_branch,
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
        write.replace_steps(ticket.id(), &steps).await?;
        write.commit().await?;
        self.signal.notify(&change);
        let row = TicketRow {
            ticket,
            created_by_name: actor.name().as_str().to_owned(),
            steps: progress(&steps),
        };
        Ok(TicketView::new(row, &index))
    }

    /// Tickets in display order: column, priority, number.
    pub async fn list(&self, filter: &TicketFilter) -> Result<Vec<TicketView>, AppError> {
        let snapshot = self.repository.list(filter.scope).await?;
        let query = filter.query.as_ref().map(|text| text.to_lowercase());
        let mut views: Vec<TicketView> = snapshot
            .rows
            .into_iter()
            .map(|row| TicketView::new(row, &snapshot.index))
            .filter(|view| filter.accepts(view, query.as_deref()))
            .collect();
        views.sort_by_cached_key(TicketView::order);
        Ok(views)
    }

    /// The ticket with its history; the key may be typed loosely, such as `arc-7`.
    pub async fn get(&self, key: &str) -> Result<TicketDetail, AppError> {
        let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
        let read = self
            .repository
            .get(&key)
            .await?
            .ok_or_else(|| not_found(&key))?;
        Ok(TicketDetail {
            view: TicketView::new(read.row, &read.index),
            history: read.history,
            steps: read.steps,
        })
    }

    /// Changes fields as legacy `update` did; a change without effect still bumps the version.
    pub async fn update(
        &self,
        key: &str,
        patch: &TicketPatch,
        actor: &Actor,
        expected: Version,
        options: WriteOptions,
    ) -> Result<TicketDetail, AppError> {
        let status = optional_status(patch.status.as_deref())?;
        let priority = match patch.priority.as_deref() {
            Some(text) => Some(Priority::parse(text).map_err(|error| invalid(&error))?),
            None => None,
        };
        if patch.is_empty() {
            return Err(nothing_changed());
        }
        let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
        let mut write = self.repository.begin().await?;
        let before = load(write.as_mut(), &key, Some(expected)).await?;
        let index = write.dependency_index().await?;
        let after = apply(write.as_mut(), &before, patch, status, priority, &index).await?;
        check_change(&before, &after, &index, options)?;
        let now = self.clock.now();
        let revised = before.revised(after, now);
        let history = self.history(&before, &revised, actor, now, None);
        let change = write.update(&revised, expected, &history).await?;
        write.commit().await?;
        self.signal.notify(&change);
        self.get(&key.to_string()).await
    }

    /// Moves to a column as legacy `move` did; leaving blocked clears the reason.
    pub async fn move_to(
        &self,
        key: &str,
        status: Option<&str>,
        acceptance: Option<&str>,
        actor: &Actor,
        expected: Option<Version>,
        options: WriteOptions,
    ) -> Result<TicketView, AppError> {
        let status = optional_status(status)?.ok_or_else(status_missing)?;
        let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
        let mut write = self.repository.begin().await?;
        let before = load(write.as_mut(), &key, expected).await?;
        let mut after = before.record().clone();
        after.status = status;
        if let Some(acceptance) = acceptance {
            after.acceptance = acceptance.to_owned();
        }
        if status != TicketStatus::Blocked {
            after.blocked_reason = None;
        }
        if status != before.status() {
            after.sort_order = write.next_sort_order(after.board, status).await?;
        }
        if changes(before.record(), &after).is_empty() {
            drop(write);
            return Ok(self.get(&key.to_string()).await?.view);
        }
        let index = write.dependency_index().await?;
        check_change(&before, &after, &index, options)?;
        let now = self.clock.now();
        let revised = before.revised(after, now);
        let history = self.history(&before, &revised, actor, now, Some("moved"));
        let change = write
            .update(&revised, before.record().version, &history)
            .await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(self.get(&key.to_string()).await?.view)
    }

    /// Deletes for good; dependents lose the dependency and a tombstone keeps key and title.
    pub async fn delete(
        &self,
        key: &str,
        actor: &Actor,
        expected: Option<Version>,
    ) -> Result<(), AppError> {
        let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
        let mut write = self.repository.begin().await?;
        let ticket = load(write.as_mut(), &key, expected).await?;
        let changes = remove(
            write.as_mut(),
            &[ticket],
            actor,
            self.clock.now(),
            self.ids.as_ref(),
        )
        .await?;
        write.commit().await?;
        if let Some(last) = changes.last() {
            self.signal.notify(last);
        }
        Ok(())
    }

    /// Appends a step or inserts it at `at`, counted from 1.
    pub async fn add_step(
        &self,
        key: &str,
        text: &str,
        detail: Option<&str>,
        at: Option<i64>,
        guard: &StepGuard,
        actor: &Actor,
    ) -> Result<TicketDetail, AppError> {
        let id = self.ids.next();
        self.change_steps(key, guard.version, actor, |steps| {
            if let Some(at) = at {
                step::expect(steps, at, guard.expect_text.as_deref())?;
            }
            Ok(vec![step::add(steps, id, text, detail, at)?])
        })
        .await
    }

    /// Edits text or guidance and checks or unchecks; a change without effect writes nothing.
    pub async fn change_step(
        &self,
        key: &str,
        id: Id<Step>,
        patch: &StepPatch,
        guard: &StepGuard,
        actor: &Actor,
    ) -> Result<TicketDetail, AppError> {
        self.change_steps(key, guard.version, actor, |steps| {
            let number = step::number(steps, id)?;
            step::expect(steps, number, guard.expect_text.as_deref())?;
            if patch.text.is_none() && patch.detail.is_none() && patch.done.is_none() {
                return Err(StepError::NothingChanged);
            }
            let mut edits = Vec::new();
            let mut current = steps.to_vec();
            if patch.text.is_some() || patch.detail.is_some() {
                let detail = patch.detail.as_ref().map(Option::as_deref);
                let edit = step::edit(&current, number, patch.text.as_deref(), detail)?;
                current.clone_from(&edit.steps);
                edits.push(edit);
            }
            if let Some(done) = patch.done {
                edits.push(step::check(&current, number, done)?);
            }
            Ok(edits)
        })
        .await
    }

    pub async fn remove_step(
        &self,
        key: &str,
        id: Id<Step>,
        guard: &StepGuard,
        actor: &Actor,
    ) -> Result<TicketDetail, AppError> {
        self.change_steps(key, guard.version, actor, |steps| {
            let number = step::number(steps, id)?;
            step::expect(steps, number, guard.expect_text.as_deref())?;
            Ok(vec![step::remove(steps, number)?])
        })
        .await
    }

    /// Moves the step to number `to`, counted from 1.
    pub async fn move_step(
        &self,
        key: &str,
        id: Id<Step>,
        to: i64,
        guard: &StepGuard,
        actor: &Actor,
    ) -> Result<TicketDetail, AppError> {
        self.change_steps(key, guard.version, actor, |steps| {
            let from = step::number(steps, id)?;
            step::expect(steps, from, guard.expect_text.as_deref())?;
            Ok(vec![step::move_step(steps, from, to)?])
        })
        .await
    }

    /// Runs step edits in one write; each edit that changes the list adds a history line.
    async fn change_steps(
        &self,
        key: &str,
        expected: Option<Version>,
        actor: &Actor,
        edits: impl FnOnce(&[Step]) -> Result<Vec<StepEdit>, StepError> + Send,
    ) -> Result<TicketDetail, AppError> {
        let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
        let mut write = self.repository.begin().await?;
        let ticket = load(write.as_mut(), &key, expected).await?;
        let current = write.steps(ticket.id()).await?;
        let edits = edits(&current).map_err(|error| step_error(&error))?;
        let mut previous = current.clone();
        let now = self.clock.now();
        let mut history = Vec::new();
        for edit in edits {
            if edit.steps == previous {
                continue;
            }
            history.push(HistoryEntry {
                id: self.ids.next(),
                ticket: ticket.id(),
                actor: actor.id(),
                at: now,
                field: "step",
                from: edit.from,
                to: edit.to,
                note: Some(edit.note),
            });
            previous = edit.steps;
        }
        if previous == current {
            drop(write);
            return self.get(&key.to_string()).await;
        }
        let revised = ticket.revised(ticket.record().clone(), now);
        let change = write
            .update(&revised, ticket.record().version, &history)
            .await?;
        write.replace_steps(ticket.id(), &previous).await?;
        write.commit().await?;
        self.signal.notify(&change);
        self.get(&key.to_string()).await
    }

    fn history(
        &self,
        before: &Ticket,
        after: &Ticket,
        actor: &Actor,
        at: Timestamp,
        note: Option<&str>,
    ) -> Vec<HistoryEntry> {
        changes(before.record(), after.record())
            .into_iter()
            .map(|change| {
                HistoryEntry::of(self.ids.next(), before.id(), actor.id(), at, change, note)
            })
            .collect()
    }
}

fn optional_status(text: Option<&str>) -> Result<Option<TicketStatus>, AppError> {
    text.map(TicketStatus::parse)
        .transpose()
        .map_err(|error| invalid(&error))
}

/// The ticket with this key, which must still have `expected` when one is given.
async fn load(
    write: &mut dyn TicketWrite,
    key: &TicketKey,
    expected: Option<Version>,
) -> Result<Ticket, AppError> {
    let ticket = write.ticket(key).await?.ok_or_else(|| not_found(key))?;
    if expected.is_some_and(|version| version != ticket.record().version) {
        return Err(AppError::version_conflict("ticket", key));
    }
    Ok(ticket)
}

/// Applies the patch field by field in the legacy order, checking each field it touches.
async fn apply(
    write: &mut dyn TicketWrite,
    before: &Ticket,
    patch: &TicketPatch,
    status: Option<TicketStatus>,
    priority: Option<Priority>,
    index: &DependencyIndex,
) -> Result<TicketRecord, AppError> {
    let old = before.record();
    let mut after = old.clone();
    if let Some(title) = &patch.title {
        after.title = TicketTitle::parse(title).map_err(|error| invalid(&error))?;
    }
    let texts = [
        (&patch.description, &mut after.description),
        (&patch.plan, &mut after.plan),
        (&patch.handbook, &mut after.handbook),
        (&patch.diagram, &mut after.diagram),
    ];
    for (given, field) in texts {
        if let Some(text) = given {
            field.clone_from(text);
        }
    }
    if let Some(status) = status {
        after.status = status;
    }
    if let Some(priority) = priority {
        after.priority = priority;
    }
    if let Some(labels) = &patch.labels {
        after.labels = normalize_labels(labels).map_err(|error| broken_rule(&error))?;
    }
    if let Some(assignee) = &patch.assignee {
        after.assignee = optional(assignee.as_deref(), ASSIGNEE_MAX, TicketError::Assignee)?;
    }
    if let Some(needs_planning) = patch.needs_planning {
        after.needs_planning = needs_planning;
    }
    if old.status == TicketStatus::Blocked
        && after.status != TicketStatus::Blocked
        && patch.blocked_reason.is_none()
    {
        after.blocked_reason = None;
    }
    if let Some(reason) = &patch.blocked_reason {
        after.blocked_reason = optional(
            reason.as_deref(),
            BLOCKED_REASON_MAX,
            TicketError::BlockedReason,
        )?;
    }
    if let Some(acceptance) = &patch.acceptance {
        after.acceptance.clone_from(acceptance);
    }
    if let Some(target) = &patch.target_branch {
        after.target_branch = match target {
            Some(text) => TargetBranch::parse(text).map_err(|error| invalid(&error))?,
            None => None,
        };
    }
    if patch.changes_dependencies() {
        let removed = dependency_keys(patch.depends_on_remove.as_deref().unwrap_or_default())?;
        // a full list wins; only without one do the added keys join the current ones
        let base = if let Some(list) = &patch.depends_on {
            dependency_keys(list)?
        } else {
            let added = dependency_keys(patch.depends_on_add.as_deref().unwrap_or_default())?;
            normalize_dependencies(old.depends_on.iter().cloned().chain(added).collect())
        };
        after.depends_on = base
            .into_iter()
            .filter(|key| !removed.contains(key))
            .collect();
        if after.depends_on != old.depends_on {
            validate_dependencies(&old.key, &after.depends_on, index.edges())
                .map_err(|error| broken_rule(&error))?;
        }
    }
    if let Some(board) = patch.board
        && board != old.board
    {
        let (target, _) = write
            .board(board)
            .await?
            .ok_or_else(|| boards::not_found(board))?;
        if target.project() != old.project {
            return Err(other_project(board));
        }
        after.board = board;
    }
    if after.status != old.status || after.board != old.board {
        after.sort_order = write.next_sort_order(after.board, after.status).await?;
    }
    Ok(after)
}

fn optional(
    text: Option<&str>,
    max: usize,
    field: fn(TextError) -> TicketError,
) -> Result<Option<String>, AppError> {
    match text {
        Some(text) => optional_text(text, max)
            .map(|text| text.map(str::to_owned))
            .map_err(|error| invalid(&field(error))),
        None => Ok(None),
    }
}

/// The legacy status locks, checked only when the status changes.
fn check_change(
    before: &Ticket,
    after: &TicketRecord,
    index: &DependencyIndex,
    options: WriteOptions,
) -> Result<(), AppError> {
    let facts = StatusFacts {
        acceptance_blank: after.acceptance.trim().is_empty(),
        open_dependencies: index.open_dependencies(&after.depends_on),
        open_questions: Vec::new(),
    };
    let request = StatusRequest {
        by_person: options.by_person,
        force: options.force,
    };
    check_status_change(Some(before.status()), after.status, request, &facts)
        .map_err(|error| locked(&error))
}

/// Deletes tickets as legacy did: dependents lose the edge, each ticket leaves a tombstone.
pub(crate) async fn remove(
    write: &mut dyn TicketWrite,
    tickets: &[Ticket],
    actor: &Actor,
    at: Timestamp,
    ids: &dyn IdGenerator,
) -> Result<Vec<Change>, AppError> {
    let deleted: HashSet<Id<Ticket>> = tickets.iter().map(Ticket::id).collect();
    let mut done = Vec::new();
    for ticket in tickets {
        for dependent in write.dependents(ticket.id()).await? {
            if deleted.contains(&dependent.id()) {
                continue;
            }
            let mut after = dependent.record().clone();
            after.depends_on.retain(|key| key != ticket.key());
            // legacy rebuilt the old list by appending the removed key at its end
            let mut shown = after.clone();
            shown.depends_on.push(ticket.key().clone());
            let revised = dependent.revised(after, at);
            let history: Vec<HistoryEntry> = changes(&shown, revised.record())
                .into_iter()
                .map(|change| {
                    HistoryEntry::of(
                        Id::from_uuid(ids.next_uuid()),
                        dependent.id(),
                        actor.id(),
                        at,
                        change,
                        Some("dependency deleted"),
                    )
                })
                .collect();
            let expected = dependent.record().version;
            done.push(write.update(&revised, expected, &history).await?);
        }
    }
    for ticket in tickets {
        let tombstone = Tombstone::of(Id::from_uuid(ids.next_uuid()), ticket, actor.id(), at);
        done.push(write.delete(ticket, &tombstone).await?);
    }
    Ok(done)
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

    #[test]
    fn dependency_keys_skip_blanks_and_repeats() {
        let texts = ["arc-1", " ", "ARC-0001", "", "zed-2"].map(String::from);
        let keys: Vec<String> = dependency_keys(&texts)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(keys, ["ARC-0001", "ZED-0002"]);
        let bad = [String::from("nope")];
        assert_eq!(
            dependency_keys(&bad).unwrap_err().code(),
            "ticket-id-invalid"
        );
    }

    #[test]
    fn columns_read_a_comma_list() {
        assert_eq!(
            columns(" waiting, In Progress ,").unwrap(),
            Some(vec![Column::Waiting, Column::InProgress])
        );
        assert_eq!(columns(" , ").unwrap(), None);
        assert_eq!(columns("later").unwrap_err().code(), "status-invalid");
    }
}
