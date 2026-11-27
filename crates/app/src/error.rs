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

/// A note that describes the same finding as one being added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimilarNote {
    pub number: i64,
    pub title: String,
    pub state: nullradix_domain::NoteState,
    /// Key of the ticket it was promoted to.
    pub ticket: Option<String>,
}

/// A refused field of an input, as manifests of packages report them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldProblem {
    pub field: String,
    pub message: String,
}

/// Boxed apart so a failure stays small.
#[derive(Debug)]
struct Extra {
    fields: Vec<FieldProblem>,
    confirmation: Option<ConfirmationFacts>,
}

/// What a person confirms before a theme package is installed, for `theme-confirmation-required`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationFacts {
    /// Everything the package holds that needs confirming.
    pub risk: nullradix_domain::ThemeRisk,
    /// What an update adds to the installed version; everything risky for a new install.
    pub added: nullradix_domain::RiskAdded,
    /// A theme with this id is installed and the package replaces it.
    pub update: bool,
}

/// Failure of a use case with a stable machine-readable code.
#[derive(Debug)]
pub struct AppError {
    kind: ErrorKind,
    code: &'static str,
    message: String,
    /// Key of the entity that already exists, for duplicates.
    existing: Option<String>,
    /// Notes that already describe the finding, for `note-similar`.
    similar: Vec<SimilarNote>,
    /// Refused fields and facts to confirm, for `theme-manifest-invalid` and the confirmation.
    extra: Option<Box<Extra>>,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl AppError {
    #[must_use]
    pub fn new(kind: ErrorKind, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
            existing: None,
            similar: Vec::new(),
            extra: None,
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

    /// The stored entity changed since the caller read it.
    #[must_use]
    pub fn version_conflict(entity: &str, id: impl std::fmt::Display) -> Self {
        Self::conflict(
            "version-conflict",
            format!("{entity} {id} was changed by someone else, reload and try again"),
        )
    }

    /// Validates a version number sent by a client.
    pub fn client_version(value: i64) -> Result<nullradix_domain::Version, Self> {
        nullradix_domain::Version::parse(value)
            .map_err(|error| Self::invalid("version-invalid", error.to_string()))
    }

    /// Refused text: `empty` names the blank field, any field too long is `value-too-long`.
    #[must_use]
    pub fn text(error: nullradix_domain::TextError, empty: &'static str, message: String) -> Self {
        let code = match error {
            nullradix_domain::TextError::Empty => empty,
            nullradix_domain::TextError::TooLong(_) => "value-too-long",
        };
        Self::invalid(code, message)
    }

    #[must_use]
    pub fn internal(error: impl Error + Send + Sync + 'static) -> Self {
        Self::new(ErrorKind::Internal, "internal", "internal error").with_source(error)
    }

    #[must_use]
    pub fn with_existing(mut self, key: String) -> Self {
        self.existing = Some(key);
        self
    }

    #[must_use]
    pub fn existing(&self) -> Option<&str> {
        self.existing.as_deref()
    }

    #[must_use]
    pub fn with_similar(mut self, notes: Vec<SimilarNote>) -> Self {
        self.similar = notes;
        self
    }

    #[must_use]
    pub fn similar(&self) -> &[SimilarNote] {
        &self.similar
    }

    #[must_use]
    pub fn with_fields(mut self, fields: Vec<FieldProblem>) -> Self {
        let confirmation = self.extra.take().and_then(|extra| extra.confirmation);
        self.extra = (!fields.is_empty() || confirmation.is_some()).then(|| {
            Box::new(Extra {
                fields,
                confirmation,
            })
        });
        self
    }

    #[must_use]
    pub fn fields(&self) -> &[FieldProblem] {
        self.extra.as_ref().map_or(&[], |extra| &extra.fields)
    }

    #[must_use]
    pub fn with_confirmation(mut self, facts: ConfirmationFacts) -> Self {
        let fields = self
            .extra
            .take()
            .map_or_else(Vec::new, |extra| extra.fields);
        self.extra = Some(Box::new(Extra {
            fields,
            confirmation: Some(facts),
        }));
        self
    }

    #[must_use]
    pub fn confirmation(&self) -> Option<&ConfirmationFacts> {
        self.extra
            .as_ref()
            .and_then(|extra| extra.confirmation.as_ref())
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
    fn invalid_input_names_its_fields() {
        let issue = FieldProblem {
            field: String::from("wip"),
            message: String::from("must be a whole number from 0"),
        };
        let error = AppError::invalid("x-invalid", "bad").with_fields(vec![issue.clone()]);
        assert_eq!(error.fields(), [issue]);
        assert_eq!(AppError::invalid("x-invalid", "bad").fields(), []);
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
