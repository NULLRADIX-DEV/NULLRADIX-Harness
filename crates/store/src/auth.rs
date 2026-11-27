use async_trait::async_trait;
use nullradix_app::auth::{
    ApiToken, AuthRepository, STEP_UP_FEED_ID, SecretHash, Session, TokenKind,
};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{Id, Timestamp};
use uuid::Uuid;

use crate::error::database;
use crate::{Store, StoreError, change_feed};

/// Key of the switch in the settings table; no row means on.
const STEP_UP_KEY: &str = "step-up";

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
            r#"SELECT id AS "id: Uuid", hash, key_hash, created_at, expires_at,
                      last_seen_at, elevated_until
               FROM sessions WHERE hash = ?"#,
            hash,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(row.map(|row| Session {
            id: Id::from_uuid(row.id),
            hash: SecretHash::from_stored(row.hash),
            key_hash: SecretHash::from_stored(row.key_hash),
            created_at: Timestamp::from_millis(row.created_at),
            expires_at: Timestamp::from_millis(row.expires_at),
            last_seen_at: Timestamp::from_millis(row.last_seen_at),
            elevated_until: row.elevated_until.map(Timestamp::from_millis),
        }))
    }

    async fn insert_session(&self, session: &Session) -> Result<(), AppError> {
        let id = session.id.as_uuid();
        let hash = session.hash.as_str();
        let key_hash = session.key_hash.as_str();
        let created_at = session.created_at.as_millis();
        let expires_at = session.expires_at.as_millis();
        let last_seen_at = session.last_seen_at.as_millis();
        let elevated_until = session.elevated_until.map(Timestamp::as_millis);
        sqlx::query!(
            "INSERT INTO sessions
                 (id, hash, key_hash, created_at, expires_at, last_seen_at, elevated_until)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            id,
            hash,
            key_hash,
            created_at,
            expires_at,
            last_seen_at,
            elevated_until,
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

    async fn delete_expired_sessions(
        &self,
        now: Timestamp,
        idle_before: Timestamp,
    ) -> Result<u64, AppError> {
        let now = now.as_millis();
        let idle_before = idle_before.as_millis();
        let result = sqlx::query!(
            "DELETE FROM sessions WHERE expires_at <= ? OR last_seen_at <= ?",
            now,
            idle_before,
        )
        .execute(&self.writer)
        .await
        .map_err(database)?;
        Ok(result.rows_affected())
    }

    async fn touch_session(&self, id: Id<Session>, at: Timestamp) -> Result<(), AppError> {
        let id = id.as_uuid();
        let at = at.as_millis();
        sqlx::query!("UPDATE sessions SET last_seen_at = ? WHERE id = ?", at, id)
            .execute(&self.writer)
            .await
            .map_err(database)?;
        Ok(())
    }

    async fn elevate_session(&self, id: Id<Session>, until: Timestamp) -> Result<bool, AppError> {
        let id = id.as_uuid();
        let until = until.as_millis();
        let result = sqlx::query!(
            "UPDATE sessions SET elevated_until = ? WHERE id = ?",
            until,
            id
        )
        .execute(&self.writer)
        .await
        .map_err(database)?;
        Ok(result.rows_affected() == 1)
    }

    async fn step_up_required(&self) -> Result<bool, AppError> {
        let stored = sqlx::query_scalar!("SELECT value FROM settings WHERE key = ?", STEP_UP_KEY)
            .fetch_optional(&self.reader)
            .await
            .map_err(database)?;
        stored.map_or(Ok(true), |text| {
            serde_json::from_str(&text)
                .map_err(|_| StoreError::Corrupt(String::from("invalid step-up switch")).into())
        })
    }

    async fn set_step_up_required(
        &self,
        required: bool,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let mut write = self.begin().await?;
        let stored = sqlx::query_scalar!("SELECT value FROM settings WHERE key = ?", STEP_UP_KEY)
            .fetch_optional(&mut *write)
            .await
            .map_err(database)?;
        let current = stored.map_or(Ok(true), |text| {
            serde_json::from_str::<bool>(&text)
                .map_err(|_| StoreError::Corrupt(String::from("invalid step-up switch")))
        })?;
        if current == required {
            return Ok(None);
        }
        let json = required.to_string();
        let millis = at.as_millis();
        sqlx::query!(
            "INSERT INTO settings (key, value, version, updated_at) VALUES (?, ?, 1, ?)
             ON CONFLICT (key) DO UPDATE
                 SET value = excluded.value, version = version + 1,
                     updated_at = excluded.updated_at",
            STEP_UP_KEY,
            json,
            millis,
        )
        .execute(&mut *write)
        .await
        .map_err(database)?;
        let change = change_feed::record(
            &mut write,
            Entity::Setting,
            STEP_UP_FEED_ID,
            ChangeOp::Update,
            at,
        )
        .await?;
        write.commit().await.map_err(database)?;
        Ok(Some(change))
    }
}
