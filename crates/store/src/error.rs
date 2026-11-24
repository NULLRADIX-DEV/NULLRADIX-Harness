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
