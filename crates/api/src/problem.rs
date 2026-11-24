use std::error::Error;

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use nullradix_app::{AppError, ErrorKind};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// RFC 9457 problem details with a stable `code` extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Problem {
    #[serde(rename = "type")]
    pub problem_type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    pub code: String,
    /// Key of the entity that already exists, such as `ARC-0007` for a duplicate ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing: Option<String>,
}

/// Error response of a handler, rendered as `application/problem+json`.
#[derive(Debug)]
pub struct ApiError(AppError);

impl From<AppError> for ApiError {
    fn from(error: AppError) -> Self {
        Self(error)
    }
}

const fn status(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Conflict => StatusCode::CONFLICT,
        ErrorKind::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let error = self.0;
        if error.kind() == ErrorKind::Internal {
            tracing::error!(code = error.code(), cause = %causes(&error), "request failed");
        }
        let mut problem = problem(status(error.kind()), error.code(), error.message());
        problem.existing = error.existing().map(str::to_owned);
        render(problem)
    }
}

fn causes(error: &dyn Error) -> String {
    let mut parts = Vec::new();
    let mut current = error.source();
    while let Some(cause) = current {
        parts.push(cause.to_string());
        current = cause.source();
    }
    parts.join(": ")
}

fn problem(status: StatusCode, code: &str, detail: &str) -> Problem {
    Problem {
        problem_type: format!("urn:nullradix:problem:{code}"),
        title: status.canonical_reason().unwrap_or_default().to_owned(),
        status: status.as_u16(),
        detail: detail.to_owned(),
        code: code.to_owned(),
        existing: None,
    }
}

fn render(problem: Problem) -> Response {
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        [(header::CONTENT_TYPE, "application/problem+json")],
        Json(problem),
    )
        .into_response()
}

pub(crate) fn response(status: StatusCode, code: &str, detail: &str) -> Response {
    render(problem(status, code, detail))
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    async fn render(error: AppError) -> (StatusCode, String, serde_json::Value) {
        let response = ApiError::from(error).into_response();
        let status = response.status();
        let content_type = response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .to_owned();
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        (status, content_type, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn renders_problem_details() {
        let (status, content_type, json) =
            render(AppError::not_found("project-not-found", "no project abc")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(content_type, "application/problem+json");
        assert_eq!(json["type"], "urn:nullradix:problem:project-not-found");
        assert_eq!(json["title"], "Not Found");
        assert_eq!(json["status"], 404);
        assert_eq!(json["detail"], "no project abc");
        assert_eq!(json["code"], "project-not-found");
        assert_eq!(json.get("existing"), None);
        let duplicate =
            AppError::conflict("ticket-duplicate", "taken").with_existing(String::from("ARC-0007"));
        let (_, _, json) = render(duplicate).await;
        assert_eq!(json["existing"], "ARC-0007");
    }

    #[tokio::test]
    async fn hides_internal_details() {
        let (status, _, json) = render(AppError::internal(std::io::Error::other("secret"))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["detail"], "internal error");
    }

    #[test]
    fn joins_the_whole_cause_chain() {
        let inner = AppError::internal(std::io::Error::other("disk on fire"));
        let outer = AppError::internal(inner);
        assert_eq!(causes(&outer), "internal: internal error: disk on fire");
    }

    #[test]
    fn maps_every_kind_to_a_status() {
        assert_eq!(status(ErrorKind::Invalid), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(status(ErrorKind::Conflict), StatusCode::CONFLICT);
        assert_eq!(status(ErrorKind::Unauthorized), StatusCode::UNAUTHORIZED);
        assert_eq!(status(ErrorKind::Forbidden), StatusCode::FORBIDDEN);
        assert_eq!(
            status(ErrorKind::Unavailable),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
