use std::error::Error;
use std::fmt;

/// Category of a failure; adapters map it to their own status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Invalid,
    NotFound,
    Conflict,
    Unauthorized,
    Forbidden,
    Unavailable,
    Internal,
}

/// Failure of a use case with a stable machine-readable code.
#[derive(Debug)]
pub struct AppError {
    kind: ErrorKind,
    code: &'static str,
    message: String,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl AppError {
    #[must_use]
    pub fn new(kind: ErrorKind, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
            source: None,
        }
    }

    #[must_use]
    pub fn invalid(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Invalid, code, message)
    }

    #[must_use]
    pub fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, code, message)
    }

    #[must_use]
    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Conflict, code, message)
    }

    #[must_use]
    pub fn unauthorized(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unauthorized, code, message)
    }

    #[must_use]
    pub fn forbidden(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Forbidden, code, message)
    }

    #[must_use]
    pub fn unavailable(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unavailable, code, message)
    }

    #[must_use]
    pub fn internal(error: impl Error + Send + Sync + 'static) -> Self {
        Self::new(ErrorKind::Internal, "internal", "internal error").with_source(error)
    }

    #[must_use]
    pub fn with_source(mut self, error: impl Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(error));
        self
    }

    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl Error for AppError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_kind_code_and_message() {
        let error = AppError::not_found("project-not-found", "project abc does not exist");
        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert_eq!(error.code(), "project-not-found");
        assert_eq!(
            error.to_string(),
            "project-not-found: project abc does not exist"
        );
    }

    #[test]
    fn internal_errors_hide_details_but_keep_the_source() {
        let error = AppError::internal(std::io::Error::other("disk on fire"));
        assert_eq!(error.message(), "internal error");
        assert_eq!(
            error.source().map(ToString::to_string).as_deref(),
            Some("disk on fire")
        );
    }
}
