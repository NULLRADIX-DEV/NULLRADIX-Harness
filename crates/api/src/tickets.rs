use axum::Json;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use nullradix_app::AppError;
use nullradix_app::auth::Principal;
use nullradix_app::comments::Author;
use nullradix_app::tickets::{
    self, HistoryLine, NewTicket, PlanInput, StepGuard, StepPatch, TicketDetail, TicketFilter,
    TicketPatch, TicketScope, TicketView, WriteOptions,
};
use nullradix_app::worktrees;
use nullradix_domain::{Column, Id, Priority, TicketKey, TicketStatus, Version};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::comments::CommentDto;
use crate::questions::OpenQuestionDto;
use crate::{API_PREFIX, ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TicketStatusDto {
    Backlog,
    InPlanning,
    Planned,
    InProgress,
    Blocked,
    Done,
    NotNeeded,
}

impl From<TicketStatus> for TicketStatusDto {
    fn from(status: TicketStatus) -> Self {
        match status {
            TicketStatus::Backlog => Self::Backlog,
            TicketStatus::InPlanning => Self::InPlanning,
            TicketStatus::Planned => Self::Planned,
            TicketStatus::InProgress => Self::InProgress,
            TicketStatus::Blocked => Self::Blocked,
            TicketStatus::Done => Self::Done,
            TicketStatus::NotNeeded => Self::NotNeeded,
        }
    }
}

impl From<TicketStatusDto> for TicketStatus {
    fn from(status: TicketStatusDto) -> Self {
        match status {
            TicketStatusDto::Backlog => Self::Backlog,
            TicketStatusDto::InPlanning => Self::InPlanning,
            TicketStatusDto::Planned => Self::Planned,
            TicketStatusDto::InProgress => Self::InProgress,
            TicketStatusDto::Blocked => Self::Blocked,
            TicketStatusDto::Done => Self::Done,
            TicketStatusDto::NotNeeded => Self::NotNeeded,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PriorityDto {
    Low,
    Normal,
    High,
    Urgent,
}

impl From<Priority> for PriorityDto {
    fn from(priority: Priority) -> Self {
        match priority {
            Priority::Low => Self::Low,
            Priority::Normal => Self::Normal,
            Priority::High => Self::High,
            Priority::Urgent => Self::Urgent,
        }
    }
}

impl From<PriorityDto> for Priority {
    fn from(priority: PriorityDto) -> Self {
        match priority {
            PriorityDto::Low => Self::Low,
            PriorityDto::Normal => Self::Normal,
            PriorityDto::High => Self::High,
            PriorityDto::Urgent => Self::Urgent,
        }
    }
}

/// A board column: the statuses plus waiting, which open dependencies derive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ColumnDto {
    Backlog,
    Waiting,
    InPlanning,
    Planned,
    InProgress,
    Blocked,
    Done,
    NotNeeded,
}

impl From<Column> for ColumnDto {
    fn from(column: Column) -> Self {
        match column {
            Column::Backlog => Self::Backlog,
            Column::Waiting => Self::Waiting,
            Column::InPlanning => Self::InPlanning,
            Column::Planned => Self::Planned,
            Column::InProgress => Self::InProgress,
            Column::Blocked => Self::Blocked,
            Column::Done => Self::Done,
            Column::NotNeeded => Self::NotNeeded,
        }
    }
}

fn keys(keys: &[TicketKey]) -> Vec<String> {
    keys.iter().map(ToString::to_string).collect()
}

/// A ticket as boards show it; timestamps are Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TicketDto {
    /// Human key such as `ARC-0007`.
    pub key: String,
    pub id: Uuid,
    pub project_id: Uuid,
    pub board_id: Uuid,
    pub number: u32,
    pub title: String,
    pub description: String,
    pub acceptance: String,
    pub plan: String,
    pub handbook: String,
    pub diagram: String,
    pub status: TicketStatusDto,
    pub priority: PriorityDto,
    pub labels: Vec<String>,
    pub depends_on: Vec<String>,
    pub assignee: Option<String>,
    pub needs_planning: bool,
    pub blocked_reason: Option<String>,
    /// Branch the ticket merges into; none means the default branch.
    pub target_branch: Option<String>,
    /// Branch of the worktree the ticket works in right now.
    pub branch: Option<String>,
    /// Folder of that worktree on its runner.
    pub worktree: Option<String>,
    pub planned_by: Option<String>,
    pub planned_at: Option<i64>,
    /// Who plans the ticket now; a claim lives 15 minutes.
    pub planning_claim_by: Option<String>,
    pub planning_claim_at: Option<i64>,
    pub created_by_id: Uuid,
    pub created_by_name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: i64,
    pub column: ColumnDto,
    /// Ahead of the work with an open dependency.
    pub waiting: bool,
    pub depends_open: Vec<String>,
    pub dependents: Vec<String>,
    /// Dependents that become ready once this ticket is done.
    pub unlocks: Vec<String>,
    pub steps_done: u32,
    pub steps_total: u32,
    /// Open questions about the ticket; they hold it back from in progress and done.
    pub open_questions: Vec<OpenQuestionDto>,
}

impl From<TicketView> for TicketDto {
    fn from(view: TicketView) -> Self {
        let record = view.ticket.record();
        Self {
            key: record.key.to_string(),
            id: record.id.as_uuid(),
            project_id: record.project.as_uuid(),
            board_id: record.board.as_uuid(),
            number: record.key.number(),
            title: record.title.as_str().to_owned(),
            description: record.description.clone(),
            acceptance: record.acceptance.clone(),
            plan: record.plan.clone(),
            handbook: record.handbook.clone(),
            diagram: record.diagram.clone(),
            status: record.status.into(),
            priority: record.priority.into(),
            labels: record.labels.clone(),
            depends_on: keys(&record.depends_on),
            assignee: record.assignee.clone(),
            needs_planning: record.needs_planning,
            blocked_reason: record.blocked_reason.clone(),
            target_branch: record
                .target_branch
                .as_ref()
                .map(|branch| branch.as_str().to_owned()),
            branch: record.branch.clone(),
            worktree: record.worktree.clone(),
            planned_by: record.planned.as_ref().map(|(name, _)| name.clone()),
            planned_at: record.planned.as_ref().map(|(_, at)| at.as_millis()),
            planning_claim_by: record.planning_claim.as_ref().map(|(name, _)| name.clone()),
            planning_claim_at: record.planning_claim.as_ref().map(|(_, at)| at.as_millis()),
            created_by_id: record.created_by.as_uuid(),
            created_by_name: view.created_by_name,
            created_at: record.created_at.as_millis(),
            updated_at: record.updated_at.as_millis(),
            version: record.version.value(),
            column: view.column.into(),
            waiting: view.column == Column::Waiting,
            depends_open: keys(&view.depends_open),
            dependents: keys(&view.dependents),
            unlocks: keys(&view.unlocks),
            steps_done: view.steps.done,
            steps_total: view.steps.total,
            open_questions: view
                .open_questions
                .into_iter()
                .map(OpenQuestionDto::from)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TicketList {
    pub items: Vec<TicketDto>,
}

/// One change in a ticket's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDto {
    pub at: i64,
    pub actor_id: Uuid,
    pub actor_name: String,
    pub field: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub note: Option<String>,
}

impl From<HistoryLine> for HistoryDto {
    fn from(line: HistoryLine) -> Self {
        Self {
            at: line.at.as_millis(),
            actor_id: line.actor.as_uuid(),
            actor_name: line.actor_name,
            field: line.field,
            from: line.from,
            to: line.to,
            note: line.note,
        }
    }
}

/// A work step; numbers count from 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StepDto {
    pub id: Uuid,
    pub number: u32,
    pub text: String,
    pub done: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TicketDetailDto {
    pub ticket: TicketDto,
    /// Oldest first.
    pub history: Vec<HistoryDto>,
    pub steps: Vec<StepDto>,
    /// Oldest first.
    pub comments: Vec<CommentDto>,
}

impl From<TicketDetail> for TicketDetailDto {
    fn from(detail: TicketDetail) -> Self {
        Self {
            ticket: detail.view.into(),
            history: detail.history.into_iter().map(HistoryDto::from).collect(),
            steps: (1..)
                .zip(detail.steps)
                .map(|(number, step)| StepDto {
                    id: step.id.as_uuid(),
                    number,
                    text: step.text,
                    done: step.done,
                    detail: step.detail,
                })
                .collect(),
            comments: detail.comments.into_iter().map(CommentDto::from).collect(),
        }
    }
}

/// A new ticket; left out fields take the defaults: backlog, normal, needs planning.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateTicket {
    pub title: String,
    pub description: Option<String>,
    pub acceptance: Option<String>,
    /// Read as text, so an unknown status is `status-invalid` as on the legacy board.
    #[schema(value_type = Option<TicketStatusDto>)]
    pub status: Option<String>,
    /// Read as text, so an unknown priority is `priority-invalid`.
    #[schema(value_type = Option<PriorityDto>)]
    pub priority: Option<String>,
    /// Entries may hold several labels split by commas.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Ticket keys, case and padding do not matter.
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub needs_planning: Option<bool>,
    pub target_branch: Option<String>,
    /// Lifts the dependency, question and not-needed locks.
    #[serde(default)]
    pub force: bool,
    /// Allows an open ticket with the same title on the board.
    #[serde(default)]
    pub allow_duplicate: bool,
    /// Texts of work steps, none done.
    #[serde(default)]
    pub steps: Vec<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct BoardTicketsPath {
    /// Project id.
    project_id: Uuid,
    /// Board id.
    board_id: Uuid,
}

/// Reads a field that may be missing, `null` or a value into a double option.
#[expect(
    clippy::option_option,
    reason = "missing keeps a field, null clears it"
)]
pub(crate) fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A change of fields; left out fields stay, `null` clears those that may be empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateTicket {
    /// Version the change is based on.
    pub version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handbook: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagram: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<TicketStatusDto>)]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<PriorityDto>)]
    pub priority: Option<String>,
    /// Replaces the labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<String>, nullable)]
    pub assignee: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_planning: Option<bool>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<String>, nullable)]
    pub blocked_reason: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<String>,
    /// Replaces the dependencies and then ignores the add list; the remove list applies last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on_add: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on_remove: Option<Vec<String>>,
    /// Moves the ticket to another board of its project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_id: Option<Uuid>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<String>, nullable)]
    pub target_branch: Option<Option<String>>,
    /// Only null, which clears both branch and worktree; a value is refused.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<String>, nullable)]
    pub branch: Option<Option<String>>,
    /// Only null, which clears both branch and worktree; a value is refused.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<String>, nullable)]
    pub worktree: Option<Option<String>>,
    /// Lifts the dependency, question and not-needed locks.
    #[serde(default)]
    pub force: bool,
    /// Name to write under, such as `claude`; the acting person when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Agent session of the caller; a name then writes as a session of that name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// A move to another column; done needs acceptance criteria, here or on the ticket.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveTicket {
    /// Required; read as text so a missing one is `status-missing` as on the legacy board.
    #[schema(value_type = Option<TicketStatusDto>)]
    pub status: Option<String>,
    /// Version the move is based on; without one the move checks none.
    pub version: Option<i64>,
    pub acceptance: Option<String>,
    /// Lifts the dependency, question and not-needed locks.
    #[serde(default)]
    pub force: bool,
    /// Name to write under, such as `claude`; the acting person when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Agent session of the caller; a name then writes as a session of that name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// A new work step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddStep {
    pub text: String,
    /// Guidance for the step; blank guidance is none.
    pub detail: Option<String>,
    /// Position from 1; without one the step goes last.
    pub at: Option<i64>,
    /// Version of the ticket the change is based on.
    pub version: Option<i64>,
    /// The text the step at `at` had when it was read.
    pub expect_text: Option<String>,
}

/// A change of one step; left out fields stay, a `null` detail removes the guidance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeStep {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<String>, nullable)]
    pub detail: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    /// The text the step had when it was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_text: Option<String>,
}

/// A move of one step to another number.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveStep {
    pub to: i64,
    pub version: Option<i64>,
    pub expect_text: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct StepPath {
    /// Ticket key such as `ARC-0007`.
    ticket_key: String,
    /// Step id; it stays while the step's number changes.
    step_id: Uuid,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct RemoveStepQuery {
    version: Option<i64>,
    /// The text the step had when it was read.
    expect_text: Option<String>,
}

fn guard(version: Option<i64>, expect_text: Option<String>) -> Result<StepGuard, AppError> {
    Ok(StepGuard {
        version: version.map(AppError::client_version).transpose()?,
        expect_text,
    })
}

/// Tickets of other boards of the same project to bring onto this board.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportTickets {
    /// 1 to 200 ticket keys; all move or none.
    pub ticket_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ImportedTickets {
    pub moved: Vec<TicketDto>,
}

/// A claim of a ticket for an agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimTicket {
    pub agent: String,
    /// In progress when left out.
    #[schema(value_type = Option<TicketStatusDto>)]
    pub status: Option<String>,
    pub version: Option<i64>,
    /// Takes the ticket from another assignee and lifts the locks.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseTicket {
    /// Applied without any lock.
    #[schema(value_type = Option<TicketStatusDto>)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptTicket {
    pub version: Option<i64>,
}

/// A plan; at least one of plan, handbook and diagram.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WritePlan {
    pub plan: Option<String>,
    pub handbook: Option<String>,
    /// Mermaid; a Markdown fence around it is dropped.
    pub diagram: Option<String>,
    pub acceptance: Option<String>,
    /// Planned when left out.
    #[schema(value_type = Option<TicketStatusDto>)]
    pub status: Option<String>,
    /// Name to write under, such as `planner-ARC-0007`; the acting person when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Agent session of the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimPlanning {
    pub agent: String,
}

/// The next ticket that needs planning, claimed for an agent when one is named.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NextPlanningClaim {
    pub agent: String,
    pub project_id: Option<Uuid>,
    pub board_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NextPlanning {
    /// None when no ticket needs planning.
    pub ticket: Option<TicketDto>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct PlanningScope {
    project_id: Option<Uuid>,
    board_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct AgentQuery {
    /// Name of the agent that holds the claim.
    agent: String,
}

fn scope(project: Option<Uuid>, board: Option<Uuid>) -> TicketScope {
    TicketScope {
        project: project.map(Id::from_uuid),
        board: board.map(Id::from_uuid),
    }
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct DeleteTicketQuery {
    /// Version the deletion is based on; without one it checks none.
    version: Option<i64>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct TicketPath {
    /// Ticket key such as `ARC-0007`; case and padding do not matter.
    pub(crate) ticket_key: String,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct TicketQuery {
    project_id: Option<Uuid>,
    board_id: Option<Uuid>,
    /// Comma-separated columns, such as `waiting,planned`.
    columns: Option<String>,
    needs_planning: Option<bool>,
    /// Exact assignee name.
    assignee: Option<String>,
    /// Exact label.
    label: Option<String>,
    /// Case-insensitive text in the key, title or description.
    query: Option<String>,
}

impl TicketQuery {
    fn into_filter(self) -> Result<TicketFilter, AppError> {
        Ok(TicketFilter {
            scope: TicketScope {
                project: self.project_id.map(Id::from_uuid),
                board: self.board_id.map(Id::from_uuid),
            },
            columns: match self.columns.as_deref() {
                Some(text) => tickets::columns(text)?,
                None => None,
            },
            needs_planning: self.needs_planning,
            assignee: self.assignee,
            label: self.label,
            query: self.query,
        })
    }
}

#[utoipa::path(
    get,
    path = "/tickets",
    tag = "tickets",
    params(TicketQuery),
    responses(
        (status = 200, description = "Tickets in display order", body = TicketList),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn list_tickets(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<TicketQuery>,
) -> Result<Json<TicketList>, ApiError> {
    let filter = query.into_filter()?;
    let items = state
        .tickets
        .list(&filter)
        .await?
        .into_iter()
        .map(TicketDto::from)
        .collect();
    Ok(Json(TicketList { items }))
}

#[utoipa::path(
    post,
    path = "/projects/{projectId}/boards/{boardId}/tickets",
    tag = "tickets",
    params(BoardTicketsPath),
    request_body = CreateTicket,
    responses(
        (status = 201, body = TicketDto,
            headers(("Location" = String, description = "URL of the new ticket"))),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "An open ticket has the same title", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn create_ticket(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<BoardTicketsPath>,
    ApiJson(input): ApiJson<CreateTicket>,
) -> Result<(StatusCode, HeaderMap, Json<TicketDto>), ApiError> {
    let options = WriteOptions {
        by_person: matches!(principal, Principal::Session { .. }),
        force: input.force,
        allow_duplicate: input.allow_duplicate,
    };
    let ticket = NewTicket {
        title: input.title,
        description: input.description,
        acceptance: input.acceptance,
        status: input.status,
        priority: input.priority,
        labels: input.labels,
        depends_on: input.depends_on,
        needs_planning: input.needs_planning,
        target_branch: input.target_branch,
        steps: input.steps,
    };
    let created = TicketDto::from(
        state
            .tickets
            .create(
                Id::from_uuid(path.project_id),
                Id::from_uuid(path.board_id),
                &ticket,
                &state.person,
                options,
            )
            .await?,
    );
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&format!("{API_PREFIX}/tickets/{}", created.key)) {
        headers.insert(header::LOCATION, value);
    }
    Ok((StatusCode::CREATED, headers, Json(created)))
}

#[utoipa::path(
    get,
    path = "/tickets/{ticketKey}",
    tag = "tickets",
    params(TicketPath),
    responses(
        (status = 200, body = TicketDetailDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn get_ticket(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
) -> Result<Json<TicketDetailDto>, ApiError> {
    Ok(Json(state.tickets.get(&path.ticket_key).await?.into()))
}

pub(crate) fn writer(principal: &Principal, force: bool) -> WriteOptions {
    WriteOptions {
        by_person: matches!(principal, Principal::Session { .. }),
        force,
        allow_duplicate: false,
    }
}

/// Longest session text a caller may send; it only tells that the write comes from a run.
const SESSION_MAX: usize = 200;

/// The actor of a write; a named author is made only for a valid request on a ticket.
pub(crate) async fn author_of(
    state: &ApiState,
    principal: &Principal,
    name: Option<&str>,
    session: Option<&str>,
    ticket_key: &str,
) -> Result<nullradix_domain::Actor, AppError> {
    if session.is_some_and(|text| text.chars().count() > SESSION_MAX) {
        return Err(AppError::text(
            nullradix_domain::TextError::TooLong(SESSION_MAX),
            "session-empty",
            String::from("the session is too long"),
        ));
    }
    let author = Author {
        by_person: matches!(principal, Principal::Session { .. }),
        name,
        session,
    };
    if !author.by_person {
        state.tickets.get(ticket_key).await?;
    }
    state.comments.author(author, &state.person).await
}

fn version(value: Option<i64>) -> Result<Option<Version>, AppError> {
    value.map(AppError::client_version).transpose()
}

#[utoipa::path(
    patch,
    path = "/tickets/{ticketKey}",
    tag = "tickets",
    params(TicketPath),
    request_body = UpdateTicket,
    responses(
        (status = 200, body = TicketDetailDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn update_ticket(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<UpdateTicket>,
) -> Result<Json<TicketDetailDto>, ApiError> {
    let link = [&input.branch, &input.worktree];
    if link.iter().any(|given| matches!(given, Some(Some(_)))) {
        return Err(worktrees::link_only_clears().into());
    }
    let clear_link = link.iter().any(|given| given.is_some());
    let expected = AppError::client_version(input.version)?;
    let options = writer(&principal, input.force);
    let actor = author_of(
        &state,
        &principal,
        input.author.as_deref(),
        input.session.as_deref(),
        &path.ticket_key,
    )
    .await?;
    let changes = TicketPatch {
        title: input.title,
        description: input.description,
        plan: input.plan,
        handbook: input.handbook,
        diagram: input.diagram,
        status: input.status,
        priority: input.priority,
        labels: input.labels,
        assignee: input.assignee,
        needs_planning: input.needs_planning,
        blocked_reason: input.blocked_reason,
        acceptance: input.acceptance,
        depends_on: input.depends_on,
        depends_on_add: input.depends_on_add,
        depends_on_remove: input.depends_on_remove,
        board: input.board_id.map(Id::from_uuid),
        target_branch: input.target_branch,
        clear_link,
    };
    let detail = state
        .tickets
        .update(&path.ticket_key, &changes, &actor, expected, options)
        .await?;
    Ok(Json(detail.into()))
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/move",
    tag = "tickets",
    params(TicketPath),
    request_body = MoveTicket,
    responses(
        (status = 200, body = TicketDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn move_ticket(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<MoveTicket>,
) -> Result<Json<TicketDto>, ApiError> {
    let actor = author_of(
        &state,
        &principal,
        input.author.as_deref(),
        input.session.as_deref(),
        &path.ticket_key,
    )
    .await?;
    let view = state
        .tickets
        .move_to(
            &path.ticket_key,
            input.status.as_deref(),
            input.acceptance.as_deref(),
            &actor,
            version(input.version)?,
            writer(&principal, input.force),
        )
        .await?;
    Ok(Json(view.into()))
}

#[utoipa::path(
    delete,
    path = "/tickets/{ticketKey}",
    tag = "tickets",
    params(TicketPath, DeleteTicketQuery),
    responses(
        (status = 204, description = "The ticket is gone, a tombstone keeps key and title"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn delete_ticket(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiQuery(query): ApiQuery<DeleteTicketQuery>,
) -> Result<StatusCode, ApiError> {
    state
        .tickets
        .delete(&path.ticket_key, &state.person, version(query.version)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/steps",
    tag = "tickets",
    params(TicketPath),
    request_body = AddStep,
    responses(
        (status = 201, body = TicketDetailDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version or a changed step", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn add_step(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<AddStep>,
) -> Result<(StatusCode, Json<TicketDetailDto>), ApiError> {
    let detail = state
        .tickets
        .add_step(
            &path.ticket_key,
            &input.text,
            input.detail.as_deref(),
            input.at,
            &guard(input.version, input.expect_text)?,
            &state.person,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(detail.into())))
}

#[utoipa::path(
    patch,
    path = "/tickets/{ticketKey}/steps/{stepId}",
    tag = "tickets",
    params(StepPath),
    request_body = ChangeStep,
    responses(
        (status = 200, body = TicketDetailDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version or a changed step", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn change_step(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<StepPath>,
    ApiJson(input): ApiJson<ChangeStep>,
) -> Result<Json<TicketDetailDto>, ApiError> {
    let step = StepPatch {
        text: input.text,
        detail: input.detail,
        done: input.done,
    };
    let detail = state
        .tickets
        .change_step(
            &path.ticket_key,
            Id::from_uuid(path.step_id),
            &step,
            &guard(input.version, input.expect_text)?,
            &state.person,
        )
        .await?;
    Ok(Json(detail.into()))
}

#[utoipa::path(
    delete,
    path = "/tickets/{ticketKey}/steps/{stepId}",
    tag = "tickets",
    params(StepPath, RemoveStepQuery),
    responses(
        (status = 200, description = "The ticket without the step", body = TicketDetailDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version or a changed step", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn remove_step(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<StepPath>,
    ApiQuery(query): ApiQuery<RemoveStepQuery>,
) -> Result<Json<TicketDetailDto>, ApiError> {
    let detail = state
        .tickets
        .remove_step(
            &path.ticket_key,
            Id::from_uuid(path.step_id),
            &guard(query.version, query.expect_text)?,
            &state.person,
        )
        .await?;
    Ok(Json(detail.into()))
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/steps/{stepId}/move",
    tag = "tickets",
    params(StepPath),
    request_body = MoveStep,
    responses(
        (status = 200, body = TicketDetailDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version or a changed step", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn move_step(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<StepPath>,
    ApiJson(input): ApiJson<MoveStep>,
) -> Result<Json<TicketDetailDto>, ApiError> {
    let detail = state
        .tickets
        .move_step(
            &path.ticket_key,
            Id::from_uuid(path.step_id),
            input.to,
            &guard(input.version, input.expect_text)?,
            &state.person,
        )
        .await?;
    Ok(Json(detail.into()))
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/claim",
    tag = "tickets",
    params(TicketPath),
    request_body = ClaimTicket,
    responses(
        (status = 200, body = TicketDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Assigned already, or a stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn claim_ticket(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<ClaimTicket>,
) -> Result<Json<TicketDto>, ApiError> {
    let view = state
        .tickets
        .claim(
            &path.ticket_key,
            &input.agent,
            input.status.as_deref(),
            &state.person,
            version(input.version)?,
            input.force,
        )
        .await?;
    Ok(Json(view.into()))
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/release",
    tag = "tickets",
    params(TicketPath),
    request_body = ReleaseTicket,
    responses(
        (status = 200, body = TicketDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn release_ticket(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<ReleaseTicket>,
) -> Result<Json<TicketDto>, ApiError> {
    let view = state
        .tickets
        .release(
            &path.ticket_key,
            input.status.as_deref(),
            &state.person,
            None,
        )
        .await?;
    Ok(Json(view.into()))
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/accept",
    tag = "tickets",
    params(TicketPath),
    request_body = AcceptTicket,
    responses(
        (status = 200, body = TicketDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn accept_ticket(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<AcceptTicket>,
) -> Result<Json<TicketDto>, ApiError> {
    let view = state
        .decisions
        .accept(
            &path.ticket_key,
            &state.person,
            version(input.version)?,
            matches!(principal, Principal::Session { .. }),
        )
        .await?;
    Ok(Json(view.into()))
}

#[utoipa::path(
    put,
    path = "/tickets/{ticketKey}/plan",
    tag = "tickets",
    params(TicketPath),
    request_body = WritePlan,
    responses(
        (status = 200, body = TicketDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn write_plan(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<WritePlan>,
) -> Result<Json<TicketDto>, ApiError> {
    let plan = PlanInput {
        plan: input.plan,
        handbook: input.handbook,
        diagram: input.diagram,
        acceptance: input.acceptance,
        status: input.status,
    };
    let actor = author_of(
        &state,
        &principal,
        input.author.as_deref(),
        input.session.as_deref(),
        &path.ticket_key,
    )
    .await?;
    let view = state.tickets.plan(&path.ticket_key, &plan, &actor).await?;
    Ok(Json(view.into()))
}

#[utoipa::path(
    post,
    path = "/tickets/{ticketKey}/planning-claim",
    tag = "tickets",
    params(TicketPath),
    request_body = ClaimPlanning,
    responses(
        (status = 200, body = TicketDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Another agent plans it", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn claim_planning(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiJson(input): ApiJson<ClaimPlanning>,
) -> Result<Json<TicketDto>, ApiError> {
    let view = state
        .tickets
        .claim_planning(&path.ticket_key, &input.agent, &state.person)
        .await?;
    Ok(Json(view.into()))
}

#[utoipa::path(
    delete,
    path = "/tickets/{ticketKey}/planning-claim",
    tag = "tickets",
    params(TicketPath, AgentQuery),
    responses(
        (status = 204, description = "The agent no longer holds a claim"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn release_planning(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<TicketPath>,
    ApiQuery(query): ApiQuery<AgentQuery>,
) -> Result<StatusCode, ApiError> {
    state
        .tickets
        .release_planning(&path.ticket_key, &query.agent, &state.person)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/planning-queue",
    tag = "tickets",
    params(PlanningScope),
    responses(
        (status = 200, description = "The next ticket that needs planning", body = NextPlanning),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn next_planning(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<PlanningScope>,
) -> Result<Json<NextPlanning>, ApiError> {
    let ticket = state
        .tickets
        .next_planning(scope(query.project_id, query.board_id), None, &state.person)
        .await?;
    Ok(Json(NextPlanning {
        ticket: ticket.map(TicketDto::from),
    }))
}

#[utoipa::path(
    post,
    path = "/planning-queue/claim",
    tag = "tickets",
    request_body = NextPlanningClaim,
    responses(
        (status = 200, description = "The claimed ticket, if any needs planning", body = NextPlanning),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn claim_next_planning(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<NextPlanningClaim>,
) -> Result<Json<NextPlanning>, ApiError> {
    let ticket = state
        .tickets
        .next_planning(
            scope(input.project_id, input.board_id),
            Some(&input.agent),
            &state.person,
        )
        .await?;
    Ok(Json(NextPlanning {
        ticket: ticket.map(TicketDto::from),
    }))
}

#[utoipa::path(
    post,
    path = "/projects/{projectId}/boards/{boardId}/ticket-imports",
    tag = "tickets",
    params(BoardTicketsPath),
    request_body = ImportTickets,
    responses(
        (status = 200, description = "The tickets on their new board", body = ImportedTickets),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn import_tickets(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<BoardTicketsPath>,
    ApiJson(input): ApiJson<ImportTickets>,
) -> Result<Json<ImportedTickets>, ApiError> {
    let moved = state
        .tickets
        .move_to_board(
            Id::from_uuid(path.project_id),
            Id::from_uuid(path.board_id),
            &input.ticket_keys,
            &state.person,
        )
        .await?;
    Ok(Json(ImportedTickets {
        moved: moved.into_iter().map(TicketDto::from).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;

    use crate::tests::{TestApi, bearer, local, open_session, send, test_api};

    async fn board_path(api: &TestApi) -> String {
        let project = api.projects.create("ARC", "Arches").await.unwrap();
        let board = api
            .boards
            .create(project.id(), "Main", &api.person)
            .await
            .unwrap()
            .board;
        format!(
            "/api/v1/projects/{}/boards/{}/tickets",
            project.id().as_uuid(),
            board.id().as_uuid()
        )
    }

    #[tokio::test]
    async fn tickets_are_created_on_a_board_and_read_by_key() {
        let api = test_api().await;
        let path = board_path(&api).await;
        let body = json!({ "title": "Fix it", "labels": ["ui, api"], "priority": "high" });
        let request = bearer(Request::post(path.as_str()), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, ticket) = send(&api.router, request).await;
        assert_eq!(status, 201);
        assert_eq!(
            (ticket["key"].as_str(), ticket["column"].as_str()),
            (Some("ARC-0001"), Some("backlog"))
        );
        assert_eq!(ticket["labels"], json!(["ui", "api"]));

        let request = bearer(Request::post(path.as_str()), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(json!({ "title": "fix IT" }).to_string()))
            .unwrap();
        let (status, _, problem) = send(&api.router, request).await;
        assert_eq!(
            (
                status.as_u16(),
                problem["code"].as_str(),
                problem["existing"].as_str()
            ),
            (409, Some("ticket-duplicate"), Some("ARC-0001"))
        );

        let request = bearer(Request::post(path.as_str()), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "title": "x", "status": "not-needed" }).to_string(),
            ))
            .unwrap();
        let (status, _, problem) = send(&api.router, request).await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (422, Some("not-needed-human-only"))
        );

        let request = bearer(Request::get("/api/v1/tickets/arc-1"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, detail) = send(&api.router, request).await;
        assert_eq!(status, 200);
        assert_eq!(detail["ticket"]["title"], "Fix it");
        assert_eq!(detail["history"][0]["note"], "created");

        let request = bearer(Request::get("/api/v1/tickets?columns=backlog"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (_, _, list) = send(&api.router, request).await;
        assert_eq!(list["items"].as_array().map(Vec::len), Some(1));
    }

    #[tokio::test]
    async fn set_and_move_inside_a_session_write_under_the_named_agent() {
        let api = test_api().await;
        let path = board_path(&api).await;
        let request = bearer(Request::post(path.as_str()), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(json!({ "title": "Fix it" }).to_string()))
            .unwrap();
        send(&api.router, request).await;
        let call = |request: axum::http::request::Builder, body: serde_json::Value| {
            bearer(request, &api.token)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let set = json!({
            "version": 1, "status": "blocked", "blockedReason": "no access",
            "author": "worker-ARC-0001", "session": "s-1"
        });
        let (status, _, detail) = send(
            &api.router,
            call(Request::patch("/api/v1/tickets/arc-1"), set),
        )
        .await;
        assert_eq!(status.as_u16(), 200, "{detail}");
        let moved = json!({ "status": "planned", "author": "worker-ARC-0001", "session": "s-1" });
        let (status, _, _) = send(
            &api.router,
            call(Request::post("/api/v1/tickets/arc-1/move"), moved),
        )
        .await;
        assert_eq!(status.as_u16(), 200);
        let anonymous = json!({ "status": "blocked", "blockedReason": "x" });
        send(
            &api.router,
            call(Request::post("/api/v1/tickets/arc-1/move"), anonymous),
        )
        .await;
        let request = bearer(Request::get("/api/v1/tickets/arc-1"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (_, _, detail) = send(&api.router, request).await;
        let actors: Vec<&str> = detail["history"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|line| line["field"] == "status")
            .filter_map(|line| line["actorName"].as_str())
            .collect();
        assert_eq!(actors.len(), 3, "{detail}");
        // newest first
        assert_eq!(&actors[1..], ["worker-ARC-0001", "worker-ARC-0001"]);
        assert_ne!(
            actors[0], "worker-ARC-0001",
            "without an author the person writes"
        );
    }

    #[tokio::test]
    async fn a_refused_write_creates_no_agent_and_a_long_session_is_refused() {
        use nullradix_app::boards::ActorRepository;
        let api = test_api().await;
        let path = board_path(&api).await;
        let request = bearer(Request::post(path.as_str()), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(json!({ "title": "Fix it" }).to_string()))
            .unwrap();
        send(&api.router, request).await;
        let call = |request: axum::http::request::Builder, body: serde_json::Value| {
            bearer(request, &api.token)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let missing = json!({ "version": 1, "status": "planned", "author": "ghost-agent" });
        let (status, _, _) = send(
            &api.router,
            call(Request::patch("/api/v1/tickets/arc-99"), missing),
        )
        .await;
        assert_eq!(status.as_u16(), 404);
        let long = json!({
            "version": 1, "status": "planned", "author": "long-agent",
            "session": "s".repeat(201)
        });
        let (status, _, problem) = send(
            &api.router,
            call(Request::patch("/api/v1/tickets/arc-1"), long),
        )
        .await;
        assert_eq!(
            (status.as_u16(), problem["code"].as_str()),
            (422, Some("value-too-long"))
        );
        for (index, name) in ["ghost-agent", "long-agent"].into_iter().enumerate() {
            let candidate = nullradix_domain::Actor::new(
                nullradix_domain::Id::from_uuid(uuid::Uuid::from_u128(77 + index as u128)),
                nullradix_domain::ActorKind::Human,
                nullradix_domain::ActorName::parse(name).unwrap(),
                nullradix_domain::Timestamp::from_millis(1),
            );
            let known = api.store.ensure(&candidate).await.unwrap();
            assert_eq!(known.id(), candidate.id(), "{name} was created");
        }
    }

    #[tokio::test]
    async fn a_browser_session_writes_as_the_person() {
        let api = test_api().await;
        let path = board_path(&api).await;
        let cookie = open_session(&api).await;
        let request = local(Request::post(path.as_str()))
            .header(axum::http::header::COOKIE, &cookie)
            .header(axum::http::header::ORIGIN, "http://127.0.0.1:4711")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "title": "Old idea", "status": "not-needed" }).to_string(),
            ))
            .unwrap();
        let (status, _, ticket) = send(&api.router, request).await;
        assert_eq!(
            (status.as_u16(), ticket["status"].as_str()),
            (201, Some("not-needed"))
        );
    }

    #[tokio::test]
    async fn bad_filters_and_fields_name_the_legacy_codes() {
        let api = test_api().await;
        let path = board_path(&api).await;
        let cases = [
            ("/api/v1/tickets?columns=bogus", None, 422, "status-invalid"),
            (
                "/api/v1/tickets?projectId=0190a0b0-0000-7000-8000-00000000dead",
                None,
                404,
                "project-not-found",
            ),
            (
                path.as_str(),
                Some(json!({ "title": "x", "status": "waiting" })),
                422,
                "status-invalid",
            ),
            (
                path.as_str(),
                Some(json!({ "title": "x", "priority": "critical" })),
                422,
                "priority-invalid",
            ),
        ];
        for (uri, body, expected, code) in cases {
            let request = match body {
                Some(body) => bearer(Request::post(uri), &api.token)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string())),
                None => bearer(Request::get(uri), &api.token).body(Body::empty()),
            }
            .unwrap();
            let (status, _, problem) = send(&api.router, request).await;
            assert_eq!(
                (status.as_u16(), problem["code"].as_str()),
                (expected, Some(code)),
                "{uri}"
            );
        }
        let request = bearer(Request::get("/api/v1/tickets?columns="), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, _) = send(&api.router, request).await;
        assert_eq!(status, 200);
    }
}
