use std::error::Error;

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use nullradix_app::{AppError, ConfirmationFacts, ErrorKind, SimilarNote};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::notes::NoteStateDto;

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
    /// Notes that already describe the finding, for `note-similar`; most similar first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub similar: Vec<SimilarNoteDto>,
    /// Refused fields of the input, for `theme-manifest-invalid`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<FieldProblemDto>,
    /// What to confirm before installing, for `theme-confirmation-required`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<ConfirmationDto>,
}

/// Code, domains and Claude part of a theme package to confirm, and what an update adds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmationDto {
    /// The package holds script files.
    pub has_code: bool,
    /// Domains the package declares in `permissions.network`.
    pub domains: Vec<String>,
    /// The package has a Claude Code theme.
    pub claude_theme: bool,
    /// The package replaces an installed theme.
    pub update: bool,
    /// What the package adds to the installed version; everything risky for a new install.
    pub added: AddedDto,
    /// Id of the theme in the package.
    pub id: String,
    /// Name of the theme in the package.
    pub name: String,
    /// Version of the package.
    pub version: String,
    /// Checksum of a catalog package; send it back as `confirmedSha256` to confirm exactly it.
    pub sha256: Option<String>,
}

/// Code, domains and Claude part added to the installed version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddedDto {
    pub code: bool,
    pub domains: Vec<String>,
    pub claude_theme: bool,
}

impl From<&ConfirmationFacts> for ConfirmationDto {
    fn from(facts: &ConfirmationFacts) -> Self {
        Self {
            has_code: facts.risk.has_code,
            domains: facts.risk.network.clone(),
            claude_theme: facts.risk.claude_theme,
            update: facts.update,
            id: facts.id.clone(),
            name: facts.name.clone(),
            version: facts.version.clone(),
            sha256: facts.sha256.clone(),
            added: AddedDto {
                code: facts.added.code,
                domains: facts.added.domains.clone(),
                claude_theme: facts.added.claude_theme,
            },
        }
    }
}

/// One refused field of an input; lists carry the index, such as `slots[1]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FieldProblemDto {
    pub field: String,
    pub message: String,
}

/// A note the similarity check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SimilarNoteDto {
    pub number: i64,
    pub title: String,
    pub state: NoteStateDto,
    /// Key of the ticket it was promoted to.
    pub ticket_key: Option<String>,
}

impl From<&SimilarNote> for SimilarNoteDto {
    fn from(note: &SimilarNote) -> Self {
        Self {
            number: note.number,
            title: note.title.clone(),
            state: note.state.into(),
            ticket_key: note.ticket.clone(),
        }
    }
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
        problem.similar = error.similar().iter().map(SimilarNoteDto::from).collect();
        problem.errors = error
            .fields()
            .iter()
            .map(|field| FieldProblemDto {
                field: field.field.clone(),
                message: field.message.clone(),
            })
            .collect();
        problem.confirmation = error.confirmation().map(ConfirmationDto::from);
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
        similar: Vec::new(),
        errors: Vec::new(),
        confirmation: None,
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
