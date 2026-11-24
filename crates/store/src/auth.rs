use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::auth::{ApiToken, AuthRepository, SecretHash, Session, TokenKind};
use nullradix_domain::{Id, Timestamp};
use uuid::Uuid;

use crate::error::database;
use crate::{Store, StoreError};

#[async_trait]
impl AuthRepository for Store {
    async fn token_by_hash(&self, hash: &SecretHash) -> Result<Option<ApiToken>, AppError> {
        let hash = hash.as_str();
        let row = sqlx::query!(
            r#"SELECT id AS "id: Uuid", name, kind, hash, created_at
               FROM api_tokens WHERE hash = ?"#,
            hash,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let kind = TokenKind::parse(&row.kind)
            .ok_or_else(|| StoreError::Corrupt(format!("unknown token kind `{}`", row.kind)))?;
        Ok(Some(ApiToken {
            id: Id::from_uuid(row.id),
            name: row.name,
            kind,
            hash: SecretHash::from_stored(row.hash),
            created_at: Timestamp::from_millis(row.created_at),
        }))
    }

    async fn replace_tokens(&self, token: &ApiToken) -> Result<(), AppError> {
        let id = token.id.as_uuid();
        let kind = token.kind.as_str();
        let hash = token.hash.as_str();
        let created_at = token.created_at.as_millis();
        let mut tx = self.begin().await?;
        sqlx::query!(
            "DELETE FROM api_tokens WHERE kind = ? AND name = ?",
            kind,
            token.name,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        sqlx::query!(
            "INSERT INTO api_tokens (id, name, kind, hash, created_at) VALUES (?, ?, ?, ?, ?)",
            id,
            token.name,
            kind,
            hash,
            created_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(())
    }

    async fn session_by_hash(&self, hash: &SecretHash) -> Result<Option<Session>, AppError> {
        let hash = hash.as_str();
        let row = sqlx::query!(
            r#"SELECT id AS "id: Uuid", hash, created_at, expires_at
               FROM sessions WHERE hash = ?"#,
            hash,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(row.map(|row| Session {
            id: Id::from_uuid(row.id),
            hash: SecretHash::from_stored(row.hash),
            created_at: Timestamp::from_millis(row.created_at),
            expires_at: Timestamp::from_millis(row.expires_at),
        }))
    }

    async fn insert_session(&self, session: &Session) -> Result<(), AppError> {
        let id = session.id.as_uuid();
        let hash = session.hash.as_str();
        let created_at = session.created_at.as_millis();
        let expires_at = session.expires_at.as_millis();
        sqlx::query!(
            "INSERT INTO sessions (id, hash, created_at, expires_at) VALUES (?, ?, ?, ?)",
            id,
            hash,
            created_at,
            expires_at,
        )
        .execute(&self.writer)
        .await
        .map_err(database)?;
        Ok(())
    }

    async fn delete_session(&self, id: Id<Session>) -> Result<(), AppError> {
        let id = id.as_uuid();
        sqlx::query!("DELETE FROM sessions WHERE id = ?", id)
            .execute(&self.writer)
            .await
            .map_err(database)?;
        Ok(())
    }

    async fn delete_expired_sessions(&self, now: Timestamp) -> Result<u64, AppError> {
        let now = now.as_millis();
        let result = sqlx::query!("DELETE FROM sessions WHERE expires_at <= ?", now)
            .execute(&self.writer)
            .await
            .map_err(database)?;
        Ok(result.rows_affected())
    }
}
