use nullradix_app::AppError;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("database migration failed")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("stored value is invalid: {0}")]
    Corrupt(String),
}

impl From<StoreError> for AppError {
    fn from(error: StoreError) -> Self {
        Self::internal(error)
    }
}

pub(crate) fn database(error: sqlx::Error) -> AppError {
    StoreError::from(error).into()
}

/// Whether `error` violates the unique constraint on `column`, written as `table.column`.
pub(crate) fn unique_violation(error: &sqlx::Error, column: &str) -> bool {
    match error {
        sqlx::Error::Database(error) => {
            error.is_unique_violation() && error.message().contains(column)
        }
        _ => false,
    }
}

/// Error for a guarded write that matched no row: a stale version or a missing entity.
pub(crate) fn missed_guard(
    exists: Result<Option<i64>, sqlx::Error>,
    conflict: AppError,
    missing: AppError,
) -> AppError {
    match exists {
        Ok(Some(_)) => conflict,
        Ok(None) => missing,
        Err(error) => database(error),
    }
}

#[cfg(test)]
mod tests {
    use nullradix_app::ErrorKind;

    use super::*;

    #[test]
    fn missed_guards_tell_conflicts_from_missing_rows() {
        let conflict = || AppError::conflict("version-conflict", "stale");
        let missing = || AppError::not_found("missing", "gone");
        assert_eq!(
            missed_guard(Ok(Some(1)), conflict(), missing()).code(),
            "version-conflict"
        );
        assert_eq!(
            missed_guard(Ok(None), conflict(), missing()).code(),
            "missing"
        );
        assert_eq!(
            missed_guard(Err(sqlx::Error::RowNotFound), conflict(), missing()).kind(),
            ErrorKind::Internal
        );
    }

    #[test]
    fn non_database_errors_are_no_unique_violations() {
        assert!(!unique_violation(&sqlx::Error::RowNotFound, "projects.key"));
    }
}
