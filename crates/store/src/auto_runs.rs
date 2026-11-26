use std::collections::HashMap;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::dispatch::tracking::{AutoRunRepository, TrackKind, Tracked};
use nullradix_domain::Timestamp;
use uuid::Uuid;

use crate::Store;
use crate::error::database;

fn row(count: i64, last: Option<i64>, auto: Option<i64>) -> Tracked {
    Tracked {
        count: u32::try_from(count).unwrap_or(u32::MAX),
        last_started_at: last.map(Timestamp::from_millis),
        auto_at: auto.map(Timestamp::from_millis),
    }
}

#[async_trait]
impl AutoRunRepository for Store {
    async fn get(&self, kind: TrackKind, subject: Uuid) -> Result<Option<Tracked>, AppError> {
        let kind = kind.as_str();
        let found = sqlx::query!(
            "SELECT count, last_started_at, auto_at FROM auto_run_tracking
             WHERE kind = ? AND subject_id = ?",
            kind,
            subject,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(found.map(|found| row(found.count, found.last_started_at, found.auto_at)))
    }

    async fn all(&self, kind: TrackKind) -> Result<HashMap<Uuid, Tracked>, AppError> {
        let kind = kind.as_str();
        let rows = sqlx::query!(
            r#"SELECT subject_id AS "subject_id: Uuid", count, last_started_at, auto_at
               FROM auto_run_tracking WHERE kind = ?"#,
            kind,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows
            .into_iter()
            .map(|found| {
                (
                    found.subject_id,
                    row(found.count, found.last_started_at, found.auto_at),
                )
            })
            .collect())
    }

    async fn started(
        &self,
        kind: TrackKind,
        subject: Uuid,
        at: Timestamp,
        auto: bool,
    ) -> Result<(), AppError> {
        let kind = kind.as_str();
        let at = at.as_millis();
        let auto_at = auto.then_some(at);
        let mut tx = self.begin().await?;
        sqlx::query!(
            "INSERT INTO auto_run_tracking (kind, subject_id, count, last_started_at, auto_at)
             VALUES (?, ?, 1, ?, ?)
             ON CONFLICT (kind, subject_id) DO UPDATE SET
               count = count + 1,
               last_started_at = excluded.last_started_at,
               auto_at = COALESCE(excluded.auto_at, auto_at)",
            kind,
            subject,
            at,
            auto_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(())
    }

    async fn mark(&self, kind: TrackKind, subject: Uuid, at: Timestamp) -> Result<(), AppError> {
        let kind = kind.as_str();
        let at = at.as_millis();
        let mut tx = self.begin().await?;
        sqlx::query!(
            "INSERT INTO auto_run_tracking (kind, subject_id, count, auto_at)
             VALUES (?, ?, 0, ?)
             ON CONFLICT (kind, subject_id) DO UPDATE SET auto_at = excluded.auto_at",
            kind,
            subject,
            at,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(())
    }

    async fn bump(&self, kind: TrackKind, subject: Uuid) -> Result<u32, AppError> {
        let kind = kind.as_str();
        let mut tx = self.begin().await?;
        let count = sqlx::query_scalar!(
            "INSERT INTO auto_run_tracking (kind, subject_id, count) VALUES (?, ?, 1)
             ON CONFLICT (kind, subject_id) DO UPDATE SET count = count + 1
             RETURNING count",
            kind,
            subject,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    async fn put(&self, kind: TrackKind, subject: Uuid, tracked: Tracked) -> Result<(), AppError> {
        let kind = kind.as_str();
        let count = i64::from(tracked.count);
        let last = tracked.last_started_at.map(Timestamp::as_millis);
        let auto = tracked.auto_at.map(Timestamp::as_millis);
        let mut tx = self.begin().await?;
        sqlx::query!(
            "INSERT INTO auto_run_tracking (kind, subject_id, count, last_started_at, auto_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (kind, subject_id) DO UPDATE SET
               count = excluded.count,
               last_started_at = excluded.last_started_at,
               auto_at = excluded.auto_at",
            kind,
            subject,
            count,
            last,
            auto,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(())
    }

    async fn clear(&self, kind: TrackKind, subject: Uuid) -> Result<(), AppError> {
        let kind = kind.as_str();
        let mut tx = self.begin().await?;
        sqlx::query!(
            "DELETE FROM auto_run_tracking WHERE kind = ? AND subject_id = ?",
            kind,
            subject,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(())
    }
}
