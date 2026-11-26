use async_trait::async_trait;
use nullradix_app::review::{ReviewRepository, StoredReview};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    Actor, Id, Review, ReviewRecord, ReviewState, Run, TestRun, Timestamp, Version, Worktree,
};
use sqlx::Row;
use uuid::Uuid;

use crate::error::database;
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid review {what}"))
}

#[async_trait]
impl ReviewRepository for Store {
    async fn hold(&self, review: &Review, tests: &[TestRun]) -> Result<Option<Change>, AppError> {
        let record = review.record();
        let id = record.id.as_uuid();
        let worktree = record.worktree.as_uuid();
        let run = record.run.as_ref().map(Id::as_uuid);
        let mut tx = self.begin().await?;
        if let Some(run) = run {
            let known: Option<i64> = sqlx::query_scalar("SELECT 1 FROM reviews WHERE run_id = ?")
                .bind(run)
                .fetch_optional(&mut *tx)
                .await
                .map_err(database)?;
            if known.is_some() {
                return Ok(None);
            }
        }
        // a later run of the same worktree replaces the review that still waits
        sqlx::query("DELETE FROM reviews WHERE worktree_id = ? AND state = 'pending'")
            .bind(worktree)
            .execute(&mut *tx)
            .await
            .map_err(database)?;
        sqlx::query(
            "INSERT INTO reviews (id, worktree_id, run_id, branch, state, created_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(worktree)
        .bind(run)
        .bind(&record.branch)
        .bind(record.state.as_str())
        .bind(record.created_at.as_millis())
        .bind(record.version.value())
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        for (position, test) in tests.iter().enumerate() {
            let position = i64::try_from(position).unwrap_or(i64::MAX);
            sqlx::query(
                "INSERT INTO review_test_runs (review_id, position, command, ok, output)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(position)
            .bind(&test.command)
            .bind(test.ok.map(i64::from))
            .bind(&test.output)
            .execute(&mut *tx)
            .await
            .map_err(database)?;
        }
        let change = change_feed::record(
            &mut tx,
            Entity::Worktree,
            worktree,
            ChangeOp::Update,
            record.created_at,
        )
        .await?;
        tx.commit().await.map_err(database)?;
        Ok(Some(change))
    }

    async fn held(&self, run: Id<Run>) -> Result<bool, AppError> {
        let known: Option<i64> = sqlx::query_scalar("SELECT 1 FROM reviews WHERE run_id = ?")
            .bind(run.as_uuid())
            .fetch_optional(&self.reader)
            .await
            .map_err(database)?;
        Ok(known.is_some())
    }

    async fn latest(&self, worktree: Id<Worktree>) -> Result<Option<StoredReview>, AppError> {
        let Some(row) = sqlx::query(
            "SELECT id, run_id, branch, state, decided_by, decided_at, created_at, version
             FROM reviews WHERE worktree_id = ? ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(worktree.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?
        else {
            return Ok(None);
        };
        let id: Uuid = row.try_get("id").map_err(database)?;
        let run: Option<Uuid> = row.try_get("run_id").map_err(database)?;
        let state: String = row.try_get("state").map_err(database)?;
        let decided_by: Option<Uuid> = row.try_get("decided_by").map_err(database)?;
        let decided_at: Option<i64> = row.try_get("decided_at").map_err(database)?;
        let record = ReviewRecord {
            id: Id::from_uuid(id),
            worktree,
            run: run.map(Id::<Run>::from_uuid),
            branch: row.try_get("branch").map_err(database)?,
            state: state.parse::<ReviewState>().map_err(|_| corrupt("state"))?,
            decided_by: decided_by.map(Id::<Actor>::from_uuid),
            decided_at: decided_at.map(Timestamp::from_millis),
            created_at: Timestamp::from_millis(row.try_get("created_at").map_err(database)?),
            version: Version::from_stored(row.try_get("version").map_err(database)?),
        };
        let rows = sqlx::query(
            "SELECT command, ok, output FROM review_test_runs WHERE review_id = ?
             ORDER BY position",
        )
        .bind(id)
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        let mut tests = Vec::with_capacity(rows.len());
        for row in &rows {
            let ok: Option<i64> = row.try_get("ok").map_err(database)?;
            tests.push(TestRun {
                command: row.try_get("command").map_err(database)?,
                ok: ok.map(|ok| ok != 0),
                output: row.try_get("output").map_err(database)?,
            });
        }
        Ok(Some(StoredReview {
            review: Review::restore(record),
            tests,
        }))
    }
}
