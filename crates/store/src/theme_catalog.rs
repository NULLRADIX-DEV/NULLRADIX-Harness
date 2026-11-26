use async_trait::async_trait;
use nullradix_app::theme_catalog::{
    SOURCES_FEED_ID, SourcesRepository, SourcesWrite, StoredSources, parse_sources, sources_json,
};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{CatalogSource, Timestamp, Version};
use sqlx::{Sqlite, SqliteConnection, Transaction};

use crate::error::database;
use crate::{Store, StoreError, change_feed};

/// Key of the catalog sources in the settings table.
const SOURCES_KEY: &str = "theme-catalog-sources";

async fn stored(connection: &mut SqliteConnection) -> Result<Option<StoredSources>, StoreError> {
    let row = sqlx::query!(
        "SELECT value, version FROM settings WHERE key = ?",
        SOURCES_KEY
    )
    .fetch_optional(&mut *connection)
    .await?;
    row.map(|row| {
        let sources = parse_sources(&row.value)
            .ok_or_else(|| StoreError::Corrupt(String::from("invalid catalog sources")))?;
        Ok(StoredSources {
            sources,
            version: Version::from_stored(row.version),
        })
    })
    .transpose()
}

struct Write(Transaction<'static, Sqlite>);

#[async_trait]
impl SourcesWrite for Write {
    async fn sources(&mut self) -> Result<Option<StoredSources>, AppError> {
        Ok(stored(&mut self.0).await?)
    }

    async fn put(
        &mut self,
        sources: &[CatalogSource],
        expected: Option<Version>,
        at: Timestamp,
    ) -> Result<(Version, Change), AppError> {
        let json = sources_json(sources).to_string();
        let millis = at.as_millis();
        let conflict = || AppError::version_conflict("catalog sources", "list");
        let (version, op) = if let Some(expected) = expected {
            let next = expected.next();
            let (next_value, expected_value) = (next.value(), expected.value());
            let updated = sqlx::query!(
                "UPDATE settings SET value = ?, version = ?, updated_at = ?
                 WHERE key = ? AND version = ?",
                json,
                next_value,
                millis,
                SOURCES_KEY,
                expected_value,
            )
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
            if updated.rows_affected() == 0 {
                return Err(conflict());
            }
            (next, ChangeOp::Update)
        } else {
            let inserted = sqlx::query!(
                "INSERT INTO settings (key, value, version, updated_at) VALUES (?, ?, 1, ?)
                 ON CONFLICT (key) DO NOTHING",
                SOURCES_KEY,
                json,
                millis,
            )
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
            if inserted.rows_affected() == 0 {
                return Err(conflict());
            }
            (Version::INITIAL, ChangeOp::Insert)
        };
        let change =
            change_feed::record(&mut self.0, Entity::Setting, SOURCES_FEED_ID, op, at).await?;
        Ok((version, change))
    }

    async fn commit(self: Box<Self>) -> Result<(), AppError> {
        self.0.commit().await.map_err(database)
    }
}

#[async_trait]
impl SourcesRepository for Store {
    async fn begin(&self) -> Result<Box<dyn SourcesWrite>, AppError> {
        Ok(Box::new(Write(self.begin().await?)))
    }

    async fn sources(&self) -> Result<Option<StoredSources>, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        Ok(stored(&mut read).await?)
    }
}
