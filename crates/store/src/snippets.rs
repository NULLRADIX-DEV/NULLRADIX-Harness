use async_trait::async_trait;
use nullradix_app::snippets::{SnippetRepository, SnippetWrite, exists, missing};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{Snippet, SnippetId, SnippetRecord, Timestamp, Version};
use sqlx::{Sqlite, SqliteConnection, Transaction};
use uuid::Uuid;

use crate::error::{database, missed_guard, unique_violation};
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid snippet {what}"))
}

struct Row {
    snippet_id: String,
    name: String,
    css: String,
    enabled: i64,
    sort_order: i64,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

impl Row {
    fn into_snippet(self) -> Result<Snippet, StoreError> {
        Ok(Snippet::restore(SnippetRecord {
            id: SnippetId::parse(&self.snippet_id).map_err(|_| corrupt("id"))?,
            name: self.name,
            css: self.css,
            enabled: self.enabled != 0,
            order: self.sort_order,
            created_at: Timestamp::from_millis(self.created_at),
            updated_at: Timestamp::from_millis(self.updated_at),
            version: Version::from_stored(self.version),
        }))
    }
}

async fn load(
    connection: &mut SqliteConnection,
    only: Option<&str>,
) -> Result<Vec<Snippet>, StoreError> {
    let rows = sqlx::query_as!(
        Row,
        "SELECT snippet_id, name, css, enabled, sort_order, created_at, updated_at, version
         FROM css_snippets
         WHERE (?1 IS NULL OR snippet_id = ?1)
         ORDER BY sort_order, snippet_id",
        only,
    )
    .fetch_all(&mut *connection)
    .await?;
    rows.into_iter().map(Row::into_snippet).collect()
}

pub(crate) struct Write(Transaction<'static, Sqlite>);

#[async_trait]
impl SnippetWrite for Write {
    async fn snippet(&mut self, id: &SnippetId) -> Result<Option<Snippet>, AppError> {
        Ok(load(&mut self.0, Some(id.as_str())).await?.pop())
    }

    async fn next_order(&mut self) -> Result<i64, AppError> {
        let next = sqlx::query_scalar!(
            r#"SELECT COALESCE(MAX(sort_order), 0) + 1 AS "next!: i64" FROM css_snippets"#
        )
        .fetch_one(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(next)
    }

    async fn insert(&mut self, snippet: &Snippet, feed_id: Uuid) -> Result<Change, AppError> {
        let record = snippet.record();
        let id = record.id.as_str();
        let created_at = record.created_at.as_millis();
        let updated_at = record.updated_at.as_millis();
        let version = record.version.value();
        let result = sqlx::query!(
            "INSERT INTO css_snippets (snippet_id, feed_id, name, css, enabled, sort_order,
                 created_at, updated_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            feed_id,
            record.name,
            record.css,
            record.enabled,
            record.order,
            created_at,
            updated_at,
            version,
        )
        .execute(&mut *self.0)
        .await;
        if let Err(error) = result {
            return Err(if unique_violation(&error, "css_snippets.snippet_id") {
                exists(&record.id)
            } else {
                database(error)
            });
        }
        Ok(change_feed::record(
            &mut self.0,
            Entity::Snippet,
            feed_id,
            ChangeOp::Insert,
            record.created_at,
        )
        .await?)
    }

    async fn update(&mut self, snippet: &Snippet, previous: Version) -> Result<Change, AppError> {
        let record = snippet.record();
        let id = record.id.as_str();
        let updated_at = record.updated_at.as_millis();
        let version = record.version.value();
        let previous = previous.value();
        let updated = sqlx::query_scalar!(
            r#"UPDATE css_snippets
               SET name = ?, css = ?, enabled = ?, updated_at = ?, version = ?
               WHERE snippet_id = ? AND version = ?
               RETURNING feed_id AS "feed_id: Uuid""#,
            record.name,
            record.css,
            record.enabled,
            updated_at,
            version,
            id,
            previous,
        )
        .fetch_optional(&mut *self.0)
        .await
        .map_err(database)?;
        let Some(feed_id) = updated else {
            let present =
                sqlx::query_scalar!("SELECT version FROM css_snippets WHERE snippet_id = ?", id)
                    .fetch_optional(&mut *self.0)
                    .await;
            return Err(missed_guard(
                present,
                AppError::version_conflict("snippet", id),
                missing(&record.id),
            ));
        };
        Ok(change_feed::record(
            &mut self.0,
            Entity::Snippet,
            feed_id,
            ChangeOp::Update,
            record.updated_at,
        )
        .await?)
    }

    async fn delete(
        &mut self,
        id: &SnippetId,
        expected: Version,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let id_text = id.as_str();
        let expected = expected.value();
        let deleted = sqlx::query_scalar!(
            r#"DELETE FROM css_snippets WHERE snippet_id = ? AND version = ?
               RETURNING feed_id AS "feed_id: Uuid""#,
            id_text,
            expected,
        )
        .fetch_optional(&mut *self.0)
        .await
        .map_err(database)?;
        let Some(feed_id) = deleted else {
            let present = sqlx::query_scalar!(
                "SELECT version FROM css_snippets WHERE snippet_id = ?",
                id_text
            )
            .fetch_optional(&mut *self.0)
            .await
            .map_err(database)?;
            return match present {
                Some(_) => Err(AppError::version_conflict("snippet", id)),
                None => Ok(None),
            };
        };
        Ok(Some(
            change_feed::record(&mut self.0, Entity::Snippet, feed_id, ChangeOp::Delete, at)
                .await?,
        ))
    }

    async fn commit(self: Box<Self>) -> Result<(), AppError> {
        self.0.commit().await.map_err(database)
    }
}

#[async_trait]
impl SnippetRepository for Store {
    async fn begin(&self) -> Result<Box<dyn SnippetWrite>, AppError> {
        Ok(Box::new(Write(self.begin().await?)))
    }

    async fn list(&self) -> Result<Vec<Snippet>, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        Ok(load(&mut read, None).await?)
    }
}
