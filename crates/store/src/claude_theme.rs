use async_trait::async_trait;
use nullradix_app::claude_theme::{CLAUDE_THEME_FEED_ID, ClaudeThemeRepository};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::Timestamp;
use sqlx::SqliteConnection;

use crate::error::database;
use crate::{Store, StoreError, change_feed};

/// Key of the switch in the settings table.
const ENABLED_KEY: &str = "claude-theme";
/// Key of the config folders that may hold the Claude part.
const TARGETS_KEY: &str = "claude-theme-targets";

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid Claude theme {what}"))
}

async fn value(connection: &mut SqliteConnection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(
        sqlx::query_scalar!("SELECT value FROM settings WHERE key = ?", key)
            .fetch_optional(&mut *connection)
            .await?,
    )
}

async fn put(
    connection: &mut SqliteConnection,
    key: &str,
    json: &str,
    at: Timestamp,
) -> Result<(), StoreError> {
    let millis = at.as_millis();
    sqlx::query!(
        "INSERT INTO settings (key, value, version, updated_at) VALUES (?, ?, 1, ?)
         ON CONFLICT (key) DO UPDATE
             SET value = excluded.value, version = version + 1,
                 updated_at = excluded.updated_at",
        key,
        json,
        millis,
    )
    .execute(&mut *connection)
    .await?;
    Ok(())
}

async fn enabled(connection: &mut SqliteConnection) -> Result<bool, StoreError> {
    value(connection, ENABLED_KEY)
        .await?
        .map_or(Ok(false), |text| {
            serde_json::from_str(&text).map_err(|_| corrupt("switch"))
        })
}

/// Turns the switch off inside a running write; `None` when it already was off.
pub(crate) async fn turn_off(
    connection: &mut SqliteConnection,
    at: Timestamp,
) -> Result<Option<Change>, StoreError> {
    if !enabled(connection).await? {
        return Ok(None);
    }
    put(connection, ENABLED_KEY, "false", at).await?;
    Ok(Some(
        change_feed::record(
            connection,
            Entity::Setting,
            CLAUDE_THEME_FEED_ID,
            ChangeOp::Update,
            at,
        )
        .await?,
    ))
}

#[async_trait]
impl ClaudeThemeRepository for Store {
    async fn enabled(&self) -> Result<bool, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        Ok(enabled(&mut read).await?)
    }

    async fn set_enabled(&self, on: bool, at: Timestamp) -> Result<Option<Change>, AppError> {
        let mut write = self.begin().await?;
        if enabled(&mut write).await? == on {
            return Ok(None);
        }
        put(&mut write, ENABLED_KEY, &on.to_string(), at).await?;
        let change = change_feed::record(
            &mut write,
            Entity::Setting,
            CLAUDE_THEME_FEED_ID,
            ChangeOp::Update,
            at,
        )
        .await?;
        write.commit().await.map_err(database)?;
        Ok(Some(change))
    }

    async fn targets(&self) -> Result<Vec<String>, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        let stored = value(&mut read, TARGETS_KEY).await?;
        Ok(stored.map_or(Ok(Vec::new()), |text| {
            serde_json::from_str(&text).map_err(|_| corrupt("folders"))
        })?)
    }

    async fn set_targets(&self, targets: &[String], at: Timestamp) -> Result<(), AppError> {
        let json = serde_json::to_string(targets).map_err(|_| corrupt("folders"))?;
        let mut write = self.begin().await?;
        if value(&mut write, TARGETS_KEY).await?.as_deref() == Some(json.as_str()) {
            return Ok(());
        }
        put(&mut write, TARGETS_KEY, &json, at).await?;
        write.commit().await.map_err(database)?;
        Ok(())
    }
}
