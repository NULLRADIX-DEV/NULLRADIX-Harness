use axum::Json;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use nullradix_app::AppError;
use nullradix_app::auth::Principal;
use nullradix_app::tickets::{
    HistoryLine, NewTicket, TicketDetail, TicketFilter, TicketScope, TicketView, WriteOptions,
};
use nullradix_domain::{Column, Id, Priority, TicketKey, TicketStatus};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TicketDetailDto {
    pub ticket: TicketDto,
    /// Oldest first.
    pub history: Vec<HistoryDto>,
}

impl From<TicketDetail> for TicketDetailDto {
    fn from(detail: TicketDetail) -> Self {
        Self {
            ticket: detail.view.into(),
            history: detail.history.into_iter().map(HistoryDto::from).collect(),
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
    pub status: Option<TicketStatusDto>,
    pub priority: Option<PriorityDto>,
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

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct TicketPath {
    /// Ticket key such as `ARC-0007`; case and padding do not matter.
    ticket_key: String,
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

fn columns(text: &str) -> Result<Vec<Column>, AppError> {
    text.split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            name.parse().map_err(|_| {
                AppError::invalid("status-invalid", format!("column {name} is unknown"))
            })
        })
        .collect()
}

impl TicketQuery {
    fn into_filter(self) -> Result<TicketFilter, AppError> {
        Ok(TicketFilter {
            scope: TicketScope {
                project: self.project_id.map(Id::from_uuid),
                board: self.board_id.map(Id::from_uuid),
            },
            columns: self.columns.as_deref().map(columns).transpose()?,
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
        status: input.status.map(TicketStatus::from),
        priority: input.priority.map(Priority::from),
        labels: input.labels,
        depends_on: input.depends_on,
        needs_planning: input.needs_planning,
        target_branch: input.target_branch,
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

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;

    use super::*;
    use crate::tests::{bearer, send, test_api};

    #[test]
    fn columns_parse_from_a_comma_list() {
        assert_eq!(
            columns(" waiting, planned ,").unwrap(),
            [Column::Waiting, Column::Planned]
        );
        assert_eq!(columns("later").unwrap_err().code(), "status-invalid");
    }

    #[tokio::test]
    async fn tickets_are_created_on_a_board_and_read_by_key() {
        let api = test_api().await;
        let project = api.projects.create("ARC", "Arches").await.unwrap();
        let board = api
            .boards
            .create(project.id(), "Main", &api.person)
            .await
            .unwrap()
            .board;
        let path = format!(
            "/api/v1/projects/{}/boards/{}/tickets",
            project.id().as_uuid(),
            board.id().as_uuid()
        );
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
}
