use async_trait::async_trait;
use nullradix_app::dispatch::log::{DispatcherLogRepository, LogLevel, LogLine};
use nullradix_app::dispatch::state::{DispatcherStateRepository, STATE_ENTITY_ID};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{DispatcherState, DispatcherStateRecord, Id, Timestamp, Version};
use sqlx::Row;
use uuid::Uuid;

use crate::error::database;
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid dispatcher {what}"))
}

#[async_trait]
impl DispatcherStateRepository for Store {
    async fn load(&self) -> Result<DispatcherState, AppError> {
        let row = sqlx::query!(
            "SELECT enabled_override, paused_until, last_pass_at, last_pass, version
             FROM dispatcher_state WHERE id = 1"
        )
        .fetch_one(&self.reader)
        .await
        .map_err(database)?;
        Ok(DispatcherState::restore(DispatcherStateRecord {
            enabled_override: row.enabled_override.map(|value| value != 0),
            paused_until: row.paused_until.map(Timestamp::from_millis),
            last_pass_at: row.last_pass_at.map(Timestamp::from_millis),
            last_pass: row.last_pass,
            version: Version::from_stored(row.version),
        }))
    }

    async fn save(
        &self,
        state: &DispatcherState,
        expected: Version,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let record = state.record();
        let enabled = record.enabled_override.map(i64::from);
        let paused = record.paused_until.map(Timestamp::as_millis);
        let last_at = record.last_pass_at.map(Timestamp::as_millis);
        let version = record.version.value();
        let expected = expected.value();
        let mut tx = self.begin().await?;
        let updated = sqlx::query!(
            "UPDATE dispatcher_state
             SET enabled_override = ?, paused_until = ?, last_pass_at = ?, last_pass = ?,
                 version = ?
             WHERE id = 1 AND version = ?",
            enabled,
            paused,
            last_at,
            record.last_pass,
            version,
            expected,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        let change = change_feed::record(
            &mut tx,
            Entity::DispatcherState,
            STATE_ENTITY_ID,
            ChangeOp::Update,
            at,
        )
        .await?;
        tx.commit().await.map_err(database)?;
        Ok(Some(change))
    }
}

#[async_trait]
impl DispatcherLogRepository for Store {
    async fn append(&self, line: &LogLine) -> Result<(), AppError> {
        let at = line.at.as_millis();
        let level = line.level.as_str();
        let run = line.run.as_ref().map(Id::as_uuid);
        let mut tx = self.begin().await?;
        sqlx::query!(
            "INSERT INTO dispatcher_log (id, at, level, message, run_id, subject_key)
             VALUES (?, ?, ?, ?, ?, ?)",
            line.id,
            at,
            level,
            line.message,
            run,
            line.subject,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(())
    }

    async fn prune_before(&self, before: Timestamp) -> Result<u64, AppError> {
        let before = before.as_millis();
        let mut tx = self.begin().await?;
        let removed = sqlx::query!("DELETE FROM dispatcher_log WHERE at < ?", before)
            .execute(&mut *tx)
            .await
            .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(removed.rows_affected())
    }

    async fn recent(&self, limit: u32) -> Result<Vec<LogLine>, AppError> {
        let rows = sqlx::query(
            "SELECT id, at, level, message, run_id, subject_key FROM dispatcher_log
             ORDER BY at DESC, id DESC LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        let mut lines = Vec::with_capacity(rows.len());
        for row in rows {
            let level: String = row.try_get("level").map_err(database)?;
            let run: Option<Uuid> = row.try_get("run_id").map_err(database)?;
            lines.push(LogLine {
                id: row.try_get("id").map_err(database)?,
                at: Timestamp::from_millis(row.try_get("at").map_err(database)?),
                level: level
                    .parse::<LogLevel>()
                    .map_err(|_| corrupt("log level"))?,
                message: row.try_get("message").map_err(database)?,
                run: run.map(Id::from_uuid),
                subject: row.try_get("subject_key").map_err(database)?,
            });
        }
        Ok(lines)
    }
}
