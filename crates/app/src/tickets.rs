//! Tickets on boards: creating them under the legacy locks and reading them in display order.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    ASSIGNEE_MAX, Actor, ActorKind, Board, BoardMoveError, ClaimError, Column, DependencyIndex,
    HistoryEntry, Id, PlanChange, Priority, Project, ProjectKey, RuleError, StatusFacts,
    StatusLock, StatusRequest, Step, StepEdit, StepError, TargetBranch, TextError, Ticket,
    TicketDraft, TicketError, TicketKey, TicketRecord, TicketStatus, TicketTitle, Timestamp,
    Tombstone, Version, changed_dependencies, changes, check_status_change, check_target_lock,
    display_order, normalize_dependencies, normalize_labels, optional_text, planning_candidate,
    step, validate_dependencies,
};

use crate::comments::{CommentLine, CommentWrite};
use crate::projects::ProjectRepository;
use crate::questions::{OpenQuestion, QuestionWrite, forget_ticket};
use crate::worktrees::WorktreeWrite;
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator, boards};

/// How many tickets one move to another board may carry, as in legacy.
pub const IMPORT_MAX: usize = 200;
/// Longest text of a plan write: plan, handbook, diagram and acceptance each.
pub const PLAN_TEXT_MAX: usize = 200_000;

/// A ticket with the name of the actor who created it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRow {
    pub ticket: Ticket,
    pub created_by_name: String,
    pub steps: StepProgress,
    /// Open questions about the ticket, by key; they hold it back from the work.
    pub open_questions: Vec<OpenQuestion>,
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
    /// Oldest first.
    pub comments: Vec<CommentLine>,
}

/// One write; dropping it without [`TicketWrite::commit`] undoes everything.
#[async_trait]
pub trait TicketWrite: WorktreeWrite + CommentWrite + QuestionWrite + Send {
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
    /// Stores the ticket, labels and dependencies if still at `expected`; not the link.
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
    pub open_questions: Vec<OpenQuestion>,
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
            open_questions: row.open_questions,
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
    /// Oldest first.
    pub comments: Vec<CommentLine>,
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
    /// Clears branch and worktree together; the worktree keeps the ticket in its list.
    pub clear_link: bool,
    /// A label an agent may not take off; it keeps a person in the loop. No change by itself.
    pub keep_label: Option<String>,
}

impl TicketPatch {
    fn is_empty(&self) -> bool {
        Self {
            keep_label: None,
            ..self.clone()
        } == Self::default()
    }

    const fn changes_dependencies(&self) -> bool {
        self.depends_on.is_some()
            || self.depends_on_add.is_some()
            || self.depends_on_remove.is_some()
    }
}

/// The fields a plan writes; at least one of plan, handbook and diagram must be given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanInput {
    pub plan: Option<String>,
    pub handbook: Option<String>,
    /// A Markdown fence around it is dropped.
    pub diagram: Option<String>,
    /// Kept as it was when blank.
    pub acceptance: Option<String>,
    /// Planned when left out.
    pub status: Option<String>,
}

/// Who writes and which legacy locks the write lifts.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts about one write"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteOptions {
    /// A person in the board, rather than an agent through the CLI.
    pub by_person: bool,
    /// Lifts the dependency, question and not-needed locks, never the acceptance lock.
    pub force: bool,
    pub allow_duplicate: bool,
    /// The request says it comes from a run or session; it may not decide for the person.
    pub from_agent: bool,
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
        TicketError::TargetBranchLocked { .. } => {
            AppError::invalid("target-branch-locked", message)
        }
    }
}

pub(crate) fn broken_rule(error: &RuleError) -> AppError {
    let code = match error {
        RuleError::LabelTooLong { .. } => "value-too-long",
        RuleError::SelfDependency(_) => "dependency-self",
        RuleError::UnknownDependency(_) => "dependency-unknown",
        RuleError::DependencyCycle(_) => "dependency-cycle",
    };
    AppError::invalid(code, error.to_string())
}

pub(crate) fn locked(error: &StatusLock) -> AppError {
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

fn agent_missing() -> AppError {
    AppError::invalid("agent-missing", "the claim needs the name of the agent")
}

fn agent_name(agent: &str) -> Result<String, AppError> {
    match optional_text(agent, ASSIGNEE_MAX) {
        Ok(Some(name)) => Ok(name.to_owned()),
        Ok(None) => Err(agent_missing()),
        Err(error) => Err(AppError::text(
            error,
            "agent-missing",
            format!("the agent name {error}"),
        )),
    }
}

fn nothing_changed() -> AppError {
    AppError::invalid("nothing-changed", "the change names no field")
}

fn status_missing() -> AppError {
    AppError::invalid("status-missing", "a move needs the status to move to")
}

fn board_move_error(error: &BoardMoveError) -> AppError {
    let code = match error {
        BoardMoveError::AlreadyThere { .. } => "ticket-already-on-board",
        BoardMoveError::OtherProject(_) => "board-other-project",
    };
    AppError::invalid(code, error.to_string())
}

fn claim_error(error: ClaimError) -> AppError {
    match error {
        ClaimError::Assigned { .. } => AppError::conflict("conflict", error.to_string()),
        ClaimError::Locked(lock) => locked(&lock),
    }
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

/// Status and priority of a new ticket, read from its input before the write starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Kind {
    pub(crate) status: TicketStatus,
    pub(crate) priority: Priority,
}

impl Kind {
    pub(crate) fn of(input: &NewTicket) -> Result<Self, AppError> {
        let status = match input.status.as_deref() {
            Some(text) => TicketStatus::parse(text).map_err(|error| invalid(&error))?,
            None => TicketStatus::Backlog,
        };
        let priority = match input.priority.as_deref() {
            Some(text) => Priority::parse(text).map_err(|error| invalid(&error))?,
            None => Priority::Normal,
        };
        Ok(Self { status, priority })
    }
}

/// Where a new ticket goes: its checked board and the key of the board's project.
pub(crate) struct Placement<'a> {
    pub(crate) board: &'a Board,
    pub(crate) project_key: ProjectKey,
    pub(crate) kind: Kind,
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

    /// Where writes begin, for use cases that change tickets together with other records.
    pub(crate) fn repository(&self) -> &dyn TicketRepository {
        self.repository.as_ref()
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
        let kind = Kind::of(input)?;
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
        let placement = Placement {
            board: &board,
            project_key,
            kind,
        };
        let (view, change) = self
            .insert_new(write.as_mut(), placement, input, actor, options)
            .await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(view)
    }

    /// Builds and inserts a new ticket in an open write whose board is checked already.
    pub(crate) async fn insert_new(
        &self,
        write: &mut dyn TicketWrite,
        placement: Placement<'_>,
        input: &NewTicket,
        actor: &Actor,
        options: WriteOptions,
    ) -> Result<(TicketView, Change), AppError> {
        let Placement {
            board,
            project_key,
            kind: Kind { status, priority },
        } = placement;
        let title = TicketTitle::parse(&input.title).map_err(|error| invalid(&error))?;
        let depends_on = dependency_keys(&input.depends_on)?;
        let steps = step::new_steps(&input.steps, || self.ids.next())
            .map_err(|error| step_error(&error))?;
        let key = TicketKey::new(project_key, write.take_number(board.project()).await?);
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
            force: options.force && !options.from_agent,
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
            board,
            draft,
            sort_order,
            actor.id(),
            self.clock.now(),
        );
        let history = [HistoryEntry::created(self.ids.next(), &ticket)];
        let change = write.insert(&ticket, &history).await?;
        write.replace_steps(ticket.id(), &steps).await?;
        let row = TicketRow {
            ticket,
            created_by_name: actor.name().as_str().to_owned(),
            steps: progress(&steps),
            open_questions: Vec::new(),
        };
        Ok((TicketView::new(row, &index), change))
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
            comments: read.comments,
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
        guard_agent_write(
            options.from_agent,
            &before,
            &after,
            patch.keep_label.as_deref(),
        )?;
        check_change(write.as_mut(), &before, &after, &index, options).await?;
        let now = self.clock.now();
        if patch.clear_link {
            write.release_link(before.id(), now).await?;
        }
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
        let mut after = before.moved(status, acceptance);
        if before.record().changes_column(&after) {
            after.sort_order = write.next_sort_order(after.board, status).await?;
        }
        if changes(before.record(), &after).is_empty() {
            drop(write);
            return Ok(self.get(&key.to_string()).await?.view);
        }
        let index = write.dependency_index().await?;
        guard_agent_write(options.from_agent, &before, &after, None)?;
        check_change(write.as_mut(), &before, &after, &index, options).await?;
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

    /// Brings tickets of other boards of the project onto `board`, all or none, as legacy did.
    pub async fn move_to_board(
        &self,
        project: Id<Project>,
        board: Id<Board>,
        keys: &[String],
        actor: &Actor,
    ) -> Result<Vec<TicketView>, AppError> {
        if keys.is_empty() || keys.len() > IMPORT_MAX {
            return Err(AppError::invalid(
                "input-invalid",
                format!("give 1 to {IMPORT_MAX} ticket keys"),
            ));
        }
        let mut write = self.repository.begin().await?;
        let target = match write.board(board).await? {
            Some((found, _)) if found.project() == project => found,
            _ => {
                drop(write);
                return Err(boards::missing(self.projects.as_ref(), project, board).await);
            }
        };
        let mut wanted = Vec::new();
        for key in keys {
            let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
            if !wanted.contains(&key) {
                wanted.push(key);
            }
        }
        let mut tickets = Vec::with_capacity(wanted.len());
        for key in &wanted {
            tickets.push(load(write.as_mut(), key, None).await?);
        }
        let mut moved_records = Vec::with_capacity(tickets.len());
        for ticket in &tickets {
            moved_records.push(
                ticket
                    .record()
                    .on_board(&target)
                    .map_err(|error| board_move_error(&error))?,
            );
        }
        let now = self.clock.now();
        let mut last = None;
        for (before, mut after) in tickets.iter().zip(moved_records) {
            let record = before.record();
            let source = match write.board(record.board).await? {
                Some((source, _)) => source.name().as_str().to_owned(),
                None => record.board.to_string(),
            };
            after.sort_order = write.next_sort_order(target.id(), record.status).await?;
            let revised = before.revised(after, now);
            let note = format!("taken from board {source}");
            let history = self.history(before, &revised, actor, now, Some(&note));
            last = Some(write.update(&revised, record.version, &history).await?);
        }
        write.commit().await?;
        if let Some(change) = &last {
            self.signal.notify(change);
        }
        let mut moved = Vec::with_capacity(wanted.len());
        for key in &wanted {
            moved.push(self.get(&key.to_string()).await?.view);
        }
        Ok(moved)
    }

    /// Takes the ticket for an agent, as legacy `claim` did; it moves to in progress unless told.
    #[allow(
        clippy::too_many_arguments,
        reason = "the route fields and who is asking"
    )]
    pub async fn claim(
        &self,
        key: &str,
        agent: &str,
        status: Option<&str>,
        actor: &Actor,
        expected: Option<Version>,
        force: bool,
        from_agent: bool,
    ) -> Result<TicketView, AppError> {
        if agent.trim().is_empty() {
            return Err(agent_missing());
        }
        // an agent's force never lifts the locks
        let force = force && !from_agent;
        let status = optional_status(status)?.unwrap_or(TicketStatus::InProgress);
        let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
        let mut write = self.repository.begin().await?;
        let name = agent_name(agent)?;
        let before = load(write.as_mut(), &key, expected).await?;
        before.claim_precheck(status, force).map_err(claim_error)?;
        let index = write.dependency_index().await?;
        let mut facts = StatusFacts::default();
        if !force {
            facts.open_dependencies = index.open_dependencies(&before.record().depends_on);
            // the question read only matters when no dependency refuses first
            if facts.open_dependencies.is_empty() {
                facts.open_questions = open_question_keys(write.as_mut(), before.id()).await?;
            }
        }
        let mut after = before
            .claimed(name, status, force, &facts)
            .map_err(claim_error)?;
        if before.record().changes_column(&after) {
            after.sort_order = write.next_sort_order(after.board, status).await?;
        }
        self.save(write, &before, after, actor, Some("claimed"), from_agent)
            .await
    }

    /// Clears the assignee; a given status applies without any lock.
    pub async fn release(
        &self,
        key: &str,
        status: Option<&str>,
        actor: &Actor,
        expected: Option<Version>,
        from_agent: bool,
    ) -> Result<TicketView, AppError> {
        let status = optional_status(status)?;
        let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
        let mut write = self.repository.begin().await?;
        let before = load(write.as_mut(), &key, expected).await?;
        let mut after = before.released(status);
        if before.record().changes_column(&after) {
            after.sort_order = write.next_sort_order(after.board, after.status).await?;
        }
        self.save(write, &before, after, actor, Some("released"), from_agent)
            .await
    }

    /// Finishes a ticket that waits for a person, as legacy `accept` did.
    pub async fn accept(
        &self,
        key: &str,
        actor: &Actor,
        expected: Option<Version>,
        by_person: bool,
    ) -> Result<TicketView, AppError> {
        let options = WriteOptions {
            by_person,
            ..WriteOptions::default()
        };
        let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
        let mut write = self.repository.begin().await?;
        let before = load(write.as_mut(), &key, expected).await?;
        let mut after = before
            .accepted()
            .map_err(|error| AppError::invalid("accept-not-possible", error.to_string()))?;
        after.sort_order = write
            .next_sort_order(after.board, TicketStatus::Done)
            .await?;
        let index = write.dependency_index().await?;
        check_change(write.as_mut(), &before, &after, &index, options).await?;
        self.save(write, &before, after, actor, Some("accepted"), false)
            .await
    }

    /// Writes the plan as legacy `plan` did: no locks, planned unless told, claim cleared.
    pub async fn plan(
        &self,
        key: &str,
        input: &PlanInput,
        actor: &Actor,
        from_agent: bool,
    ) -> Result<TicketView, AppError> {
        if input.plan.is_none() && input.handbook.is_none() && input.diagram.is_none() {
            return Err(AppError::invalid(
                "plan-empty",
                "give at least a plan, a handbook or a diagram",
            ));
        }
        for (name, text) in [
            ("plan", &input.plan),
            ("handbook", &input.handbook),
            ("diagram", &input.diagram),
            ("acceptance", &input.acceptance),
        ] {
            if text
                .as_ref()
                .is_some_and(|text| text.chars().count() > PLAN_TEXT_MAX)
            {
                return Err(AppError::text(
                    nullradix_domain::TextError::TooLong(PLAN_TEXT_MAX),
                    "plan-empty",
                    format!("the {name} has more than {PLAN_TEXT_MAX} characters"),
                ));
            }
        }
        let status = optional_status(input.status.as_deref())?.unwrap_or(TicketStatus::Planned);
        if actor.kind() != ActorKind::Human
            && !matches!(
                status,
                TicketStatus::Planned | TicketStatus::Blocked | TicketStatus::InPlanning
            )
        {
            return Err(AppError::invalid(
                "plan-status-denied",
                format!(
                    "an agent plans only into planned, blocked or in-planning, not {}",
                    status.as_str()
                ),
            ));
        }
        let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
        let mut write = self.repository.begin().await?;
        let before = load(write.as_mut(), &key, None).await?;
        let record = before.record();
        let mut after = before.planned(&PlanChange {
            plan: input.plan.as_deref(),
            handbook: input.handbook.as_deref(),
            diagram: input.diagram.as_deref(),
            acceptance: input.acceptance.as_deref(),
            status,
            by: actor.name().as_str(),
            at: self.clock.now(),
        });
        if record.changes_column(&after) {
            after.sort_order = write.next_sort_order(after.board, status).await?;
        }
        let note = if status == record.status {
            "plan updated"
        } else {
            "planned"
        };
        self.save(write, &before, after, actor, Some(note), from_agent)
            .await
    }

    /// Claims planning for 15 minutes; another agent's live claim refuses it.
    pub async fn claim_planning(
        &self,
        key: &str,
        agent: &str,
        actor: &Actor,
        from_agent: bool,
    ) -> Result<TicketView, AppError> {
        if agent.trim().is_empty() {
            return Err(agent_missing());
        }
        let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
        let mut write = self.repository.begin().await?;
        let name = agent_name(agent)?;
        let before = load(write.as_mut(), &key, None).await?;
        let after = before
            .planning_claimed(name, self.clock.now())
            .map_err(|error| AppError::conflict("plan-claimed", error.to_string()))?;
        self.save(write, &before, after, actor, None, from_agent)
            .await
    }

    /// Gives up the planning claim; a claim of someone else stays without a word.
    pub async fn release_planning(
        &self,
        key: &str,
        agent: &str,
        actor: &Actor,
        from_agent: bool,
    ) -> Result<(), AppError> {
        let key = TicketKey::parse(key).map_err(|error| invalid(&error))?;
        let mut write = self.repository.begin().await?;
        let before = load(write.as_mut(), &key, None).await?;
        let Some(after) = before.planning_released(agent) else {
            return Ok(());
        };
        self.save(write, &before, after, actor, None, from_agent)
            .await?;
        Ok(())
    }

    /// The first ticket in display order that needs planning; with an agent it is claimed.
    pub async fn next_planning(
        &self,
        scope: TicketScope,
        agent: Option<&str>,
        actor: &Actor,
        from_agent: bool,
    ) -> Result<Option<TicketView>, AppError> {
        if agent.is_some_and(|agent| agent.trim().is_empty()) {
            return Err(agent_missing());
        }
        let now = self.clock.now();
        let candidates: Vec<TicketView> = self
            .list(&TicketFilter {
                scope,
                ..TicketFilter::default()
            })
            .await?
            .into_iter()
            .filter(|view| {
                view.open_questions.is_empty() && planning_candidate(view.ticket.record(), now)
            })
            .collect();
        let Some(agent) = agent else {
            return Ok(candidates.into_iter().next());
        };
        let name = agent_name(agent)?;
        for candidate in candidates {
            let mut write = self.repository.begin().await?;
            // another agent may have claimed it since the list was read
            let Some(before) = write.ticket(candidate.ticket.key()).await? else {
                continue;
            };
            // and it must still sit where the caller looked
            let record = before.record();
            let in_scope = scope.board.is_none_or(|board| record.board == board)
                && scope
                    .project
                    .is_none_or(|project| record.project == project);
            if !in_scope
                || !planning_candidate(record, now)
                || !write.open_questions(before.id()).await?.is_empty()
            {
                continue;
            }
            let Ok(after) = before.planning_claimed(name.clone(), now) else {
                continue;
            };
            return Ok(Some(
                self.save(write, &before, after, actor, None, from_agent)
                    .await?,
            ));
        }
        Ok(None)
    }

    /// Stores the next version with its history and returns the ticket as boards show it.
    async fn save(
        &self,
        mut write: Box<dyn TicketWrite>,
        before: &Ticket,
        after: TicketRecord,
        actor: &Actor,
        note: Option<&str>,
        from_agent: bool,
    ) -> Result<TicketView, AppError> {
        guard_agent_write(from_agent, before, &after, None)?;
        let now = self.clock.now();
        let revised = before.revised(after, now);
        let history = self.history(before, &revised, actor, now, note);
        let change = write
            .update(&revised, before.record().version, &history)
            .await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(self.get(&revised.key().to_string()).await?.view)
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

    pub(crate) fn history(
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
pub(crate) async fn load(
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
    after.blocked_reason = old
        .blocked_reason_after(
            after.status,
            patch.blocked_reason.as_ref().map(Option::as_deref),
        )
        .map_err(|error| invalid(&error))?;
    if let Some(acceptance) = &patch.acceptance {
        after.acceptance.clone_from(acceptance);
    }
    if patch.clear_link {
        after.clear_link();
    }
    if let Some(target) = &patch.target_branch {
        after.target_branch = match target {
            Some(text) => TargetBranch::parse(text).map_err(|error| invalid(&error))?,
            None => None,
        };
        check_target_lock(old, &after).map_err(|error| invalid(&error))?;
    }
    if patch.changes_dependencies() {
        let removed = dependency_keys(patch.depends_on_remove.as_deref().unwrap_or_default())?;
        let list = patch
            .depends_on
            .as_deref()
            .map(dependency_keys)
            .transpose()?;
        let added = if list.is_some() {
            Vec::new()
        } else {
            dependency_keys(patch.depends_on_add.as_deref().unwrap_or_default())?
        };
        after.depends_on = changed_dependencies(&old.depends_on, list, added, &removed);
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
        after.board = old
            .on_board(&target)
            .map_err(|error| board_move_error(&error))?
            .board;
    }
    if old.changes_column(&after) {
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

/// Keys of the open questions about the ticket.
pub(crate) async fn open_question_keys(
    write: &mut dyn TicketWrite,
    ticket: Id<Ticket>,
) -> Result<Vec<String>, AppError> {
    Ok(write
        .open_questions(ticket)
        .await?
        .iter()
        .map(|question| question.key.to_string())
        .collect())
}

/// The legacy status locks, checked only when the status changes.
async fn check_change(
    write: &mut dyn TicketWrite,
    before: &Ticket,
    after: &TicketRecord,
    index: &DependencyIndex,
    options: WriteOptions,
) -> Result<(), AppError> {
    let facts = StatusFacts {
        acceptance_blank: after.acceptance.trim().is_empty(),
        open_dependencies: index.open_dependencies(&after.depends_on),
        open_questions: open_question_keys(write, before.id()).await?,
    };
    let request = StatusRequest {
        by_person: options.by_person,
        force: options.force && !options.from_agent,
    };
    check_status_change(Some(before.status()), after.status, request, &facts)
        .map_err(|error| locked(&error))
}

/// The rule for every write an agent makes: it may not end or bypass a wait for the person.
pub(crate) fn guard_agent_write(
    from_agent: bool,
    before: &Ticket,
    after: &TicketRecord,
    keep_label: Option<&str>,
) -> Result<(), AppError> {
    if !from_agent {
        return Ok(());
    }
    nullradix_domain::check_agent_write(before.record(), after, keep_label)
        .map_err(|refusal| crate::auth::decision_needs_person(&refusal.to_string()))
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
        done.extend(forget_ticket(write, ticket, at).await?);
        // the worktree keeps the ticket in its list; only the live link ends
        if let Some(change) = write.release_link(ticket.id(), at).await? {
            done.push(change);
        }
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
                invalid(&TicketError::TargetBranchLocked {
                    key: String::from("ARC-0001"),
                    branch: String::from("ticket/ARC-0001-a"),
                }),
                "target-branch-locked",
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
