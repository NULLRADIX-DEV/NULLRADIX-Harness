use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use nullradix_app::AppError;
use nullradix_app::inbox::{InboxList as List, OrderEntry};
use nullradix_domain::{Id, InboxCounts as Counts, InboxItem, InboxKind, InboxRef};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::tickets::PriorityDto;
use crate::{ApiError, ApiJson, ApiQuery, ApiState, Problem};

/// Kind of an entry; the order is the urgency, merge first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum InboxKindDto {
    /// The merge of the ticket's branch failed.
    Merge,
    /// A worktree setup step failed.
    Setup,
    Question,
    /// Blocked with `Human Interaction:`, waiting to be accepted.
    Accept,
    Blocked,
    Planning,
    Note,
}

impl From<InboxKind> for InboxKindDto {
    fn from(kind: InboxKind) -> Self {
        match kind {
            InboxKind::Merge => Self::Merge,
            InboxKind::Setup => Self::Setup,
            InboxKind::Question => Self::Question,
            InboxKind::Accept => Self::Accept,
            InboxKind::Blocked => Self::Blocked,
            InboxKind::Planning => Self::Planning,
            InboxKind::Note => Self::Note,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum InboxItemTypeDto {
    Ticket,
    Question,
    Note,
}

impl InboxItemTypeDto {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ticket => "ticket",
            Self::Question => "question",
            Self::Note => "note",
        }
    }
}

/// What an entry points at, such as ticket `ARC-0001` or note `7`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InboxRefDto {
    #[serde(rename = "type")]
    pub item_type: InboxItemTypeDto,
    pub key: String,
}

impl From<&InboxRef> for InboxRefDto {
    fn from(item: &InboxRef) -> Self {
        let item_type = match item {
            InboxRef::Ticket(_) => InboxItemTypeDto::Ticket,
            InboxRef::Question(_) => InboxItemTypeDto::Question,
            InboxRef::Note(_) => InboxItemTypeDto::Note,
        };
        Self {
            item_type,
            key: item.key(),
        }
    }
}

/// One entry of the inbox; `sinceAt` is Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxItemDto {
    #[serde(rename = "type")]
    pub item_type: InboxItemTypeDto,
    /// Ticket key, question key or note number.
    pub key: String,
    pub kind: InboxKindDto,
    pub title: String,
    /// Block reason, occasion of the question or the rest of the note.
    pub detail: Option<String>,
    pub board_id: Option<Uuid>,
    pub board_name: Option<String>,
    pub project_key: String,
    /// Questions only: open tickets waiting for the answer.
    pub waiting_tickets: u32,
    /// Tickets only.
    pub priority: Option<PriorityDto>,
    /// Last change of a ticket, creation of a question or note.
    pub since_at: i64,
    /// Urgency of the kind, 1 is the most urgent.
    pub rank: u32,
}

impl From<InboxItem> for InboxItemDto {
    fn from(item: InboxItem) -> Self {
        let reference = InboxRefDto::from(&item.item);
        Self {
            item_type: reference.item_type,
            key: reference.key,
            kind: item.kind.into(),
            rank: item.rank(),
            title: item.title,
            detail: item.detail,
            board_id: item.board.map(|board| board.as_uuid()),
            board_name: item.board_name,
            project_key: item.project_key.to_string(),
            waiting_tickets: item.waiting_tickets,
            priority: item.priority.map(Into::into),
            since_at: item.since.as_millis(),
        }
    }
}

/// Entries of each kind, before the kind filter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct InboxCounts {
    pub merge: u32,
    pub setup: u32,
    pub question: u32,
    pub accept: u32,
    pub blocked: u32,
    pub planning: u32,
    pub note: u32,
}

impl From<Counts> for InboxCounts {
    fn from(counts: Counts) -> Self {
        Self {
            merge: counts.merge,
            setup: counts.setup,
            question: counts.question,
            accept: counts.accept,
            blocked: counts.blocked,
            planning: counts.planning,
            note: counts.note,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxList {
    pub items: Vec<InboxItemDto>,
    pub counts: InboxCounts,
    /// Entries of every kind.
    pub total: u32,
    /// Version of the saved order; send it back when saving or resetting the order.
    pub order_version: i64,
}

impl From<List> for InboxList {
    fn from(list: List) -> Self {
        Self {
            items: list.items.into_iter().map(InboxItemDto::from).collect(),
            counts: list.counts.into(),
            total: list.total,
            order_version: list.order_version.value(),
        }
    }
}

/// The order of the inbox, first entry on top; entries not named come above it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InboxOrder {
    /// Repeats, entries that do not exist and entries beyond the first 1000 are dropped.
    pub items: Vec<InboxRefDto>,
    /// The `orderVersion` the order is based on; a stale one fails with `version-conflict`.
    pub version: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct OrderVersionQuery {
    /// The `orderVersion` the reset is based on.
    version: i64,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct InboxQuery {
    /// Only this board; every board when left out.
    board_id: Option<Uuid>,
    /// Only these kinds, split by commas, such as `merge,accept`; counts cover all kinds.
    kind: Option<String>,
}

#[utoipa::path(
    get,
    path = "/me/inbox",
    tag = "inbox",
    params(InboxQuery),
    responses(
        (status = 200, description = "What waits for the person, in their order", body = InboxList),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn get_inbox(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<InboxQuery>,
) -> Result<Json<InboxList>, ApiError> {
    let list = state
        .inbox
        .list(query.board_id.map(Id::from_uuid), query.kind.as_deref())
        .await?;
    Ok(Json(list.into()))
}

#[utoipa::path(
    put,
    path = "/me/inbox/order",
    tag = "inbox",
    request_body = InboxOrder,
    responses(
        (status = 200, description = "The stored order", body = InboxOrder),
        (status = 401, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn save_inbox_order(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<InboxOrder>,
) -> Result<Json<InboxOrder>, ApiError> {
    let entries: Vec<OrderEntry<'_>> = input
        .items
        .iter()
        .map(|item| OrderEntry {
            item_type: item.item_type.as_str(),
            key: &item.key,
        })
        .collect();
    let saved = state
        .inbox
        .save_order(&entries, AppError::client_version(input.version)?)
        .await?;
    Ok(Json(InboxOrder {
        items: saved.order.iter().map(InboxRefDto::from).collect(),
        version: saved.version.value(),
    }))
}

#[utoipa::path(
    delete,
    path = "/me/inbox/order",
    tag = "inbox",
    params(OrderVersionQuery),
    responses(
        (status = 204, description = "Back to the default order"),
        (status = 401, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn clear_inbox_order(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<OrderVersionQuery>,
) -> Result<StatusCode, ApiError> {
    state
        .inbox
        .clear_order(AppError::client_version(query.version)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;

    use crate::tests::{bearer, send, test_api};

    #[tokio::test]
    async fn lists_saves_and_clears_the_order() {
        let api = test_api().await;
        let get = |path: &str| {
            bearer(Request::get(path), &api.token)
                .body(Body::empty())
                .unwrap()
        };
        let (status, _, json) = send(&api.router, get("/api/v1/me/inbox")).await;
        assert_eq!(status, 200);
        assert_eq!(json["total"], 0);
        assert_eq!(json["counts"]["merge"], 0);
        assert_eq!(json["orderVersion"], 1);
        let (status, _, json) = send(&api.router, get("/api/v1/me/inbox?kind=bogus")).await;
        assert_eq!(status, 422);
        assert_eq!(json["code"], "inbox-kind-invalid");
        let board = "/api/v1/me/inbox?boardId=0190a0b0-0000-7000-8000-000000000001";
        let (status, _, json) = send(&api.router, get(board)).await;
        assert_eq!(status, 404);
        assert_eq!(json["code"], "board-not-found");

        let put = |body: &str| {
            bearer(Request::put("/api/v1/me/inbox/order"), &api.token)
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        let (status, _, json) = send(
            &api.router,
            put(r#"{"items":[{"type":"note","key":"7"}],"version":1}"#),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(json["items"], serde_json::json!([]));
        assert_eq!(json["version"], 2);
        let (status, _, json) = send(&api.router, get("/api/v1/me/inbox")).await;
        assert_eq!(status, 200);
        assert_eq!(json["orderVersion"], 2);
        let (status, _, json) = send(
            &api.router,
            put(r#"{"items":[{"type":"note","key":"7"}],"version":1}"#),
        )
        .await;
        assert_eq!(status, 409);
        assert_eq!(json["code"], "version-conflict");
        let (status, _, json) = send(
            &api.router,
            put(r#"{"items":[{"type":"ticket","key":"seven"}],"version":2}"#),
        )
        .await;
        assert_eq!(status, 422);
        assert_eq!(json["code"], "inbox-key-invalid");
        let (status, _, json) = send(&api.router, put(r#"{"items":[],"version":0}"#)).await;
        assert_eq!(status, 422);
        assert_eq!(json["code"], "version-invalid");

        let delete = |version: u32| {
            bearer(
                Request::delete(format!("/api/v1/me/inbox/order?version={version}")),
                &api.token,
            )
            .body(Body::empty())
            .unwrap()
        };
        let (status, _, json) = send(&api.router, delete(1)).await;
        assert_eq!(status, 409);
        assert_eq!(json["code"], "version-conflict");
        let (status, _, _) = send(&api.router, delete(2)).await;
        assert_eq!(status, 204);

        let changes = api.changes.changes_since(0, 100).await.unwrap();
        let orders = changes
            .iter()
            .filter(|change| change.entity == nullradix_app::Entity::InboxOrder)
            .count();
        assert_eq!(orders, 2, "saving and resetting both reach the change feed");
    }
}
