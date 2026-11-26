use async_trait::async_trait;
use nullradix_app::review::{ReviewRepository, StoredReview};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    Actor, Id, Review, ReviewRecord, ReviewState, Run, TestRun, Ticket, Timestamp, Version,
    Worktree,
};
use uuid::Uuid;

use crate::error::database;
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid review {what}"))
}

struct ReviewRow {
    id: Uuid,
    run_id: Option<Uuid>,
    branch: String,
    state: String,
    decided_by: Option<Uuid>,
    decided_at: Option<i64>,
    created_at: i64,
    version: i64,
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
            let known = sqlx::query_scalar!("SELECT 1 FROM reviews WHERE run_id = ?", run)
                .fetch_optional(&mut *tx)
                .await
                .map_err(database)?;
            if known.is_some() {
                return Ok(None);
            }
        }
        // a later run of the same worktree replaces the review that still waits
        sqlx::query!(
            "DELETE FROM reviews WHERE worktree_id = ? AND state = 'pending'",
            worktree
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        let state = record.state.as_str();
        let created_at = record.created_at.as_millis();
        let version = record.version.value();
        sqlx::query!(
            "INSERT INTO reviews (id, worktree_id, run_id, branch, state, created_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            id,
            worktree,
            run,
            record.branch,
            state,
            created_at,
            version,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        for (position, test) in tests.iter().enumerate() {
            let position = i64::try_from(position).unwrap_or(i64::MAX);
            let ok = test.ok.map(i64::from);
            sqlx::query!(
                "INSERT INTO review_test_runs (review_id, position, command, ok, output)
                 VALUES (?, ?, ?, ?, ?)",
                id,
                position,
                test.command,
                ok,
                test.output,
            )
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
        let run = run.as_uuid();
        let known = sqlx::query_scalar!("SELECT 1 FROM reviews WHERE run_id = ?", run)
            .fetch_optional(&self.reader)
            .await
            .map_err(database)?;
        Ok(known.is_some())
    }

    async fn blocked(&self, run: Id<Run>) -> Result<bool, AppError> {
        let run = run.as_uuid();
        let known = sqlx::query_scalar!(
            "SELECT 1 FROM reviews WHERE run_id = ? AND blocked_at IS NOT NULL",
            run
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(known.is_some())
    }

    async fn mark_blocked(&self, run: Id<Run>, at: Timestamp) -> Result<(), AppError> {
        let run = run.as_uuid();
        let at = at.as_millis();
        sqlx::query!(
            "UPDATE reviews SET blocked_at = ? WHERE run_id = ? AND blocked_at IS NULL",
            at,
            run
        )
        .execute(&self.writer)
        .await
        .map_err(database)?;
        Ok(())
    }

    async fn decide(
        &self,
        ticket: Id<Ticket>,
        state: ReviewState,
        by: Id<Actor>,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let ticket = ticket.as_uuid();
        let state = state.as_str();
        let by = by.as_uuid();
        let at = at.as_millis();
        let mut tx = self.begin().await?;
        let found = sqlx::query!(
            r#"SELECT r.id AS "id: Uuid", r.worktree_id AS "worktree_id: Uuid"
               FROM reviews r JOIN worktree_tickets t ON t.worktree_id = r.worktree_id
               WHERE t.ticket_id = ? AND r.state = 'pending' LIMIT 1"#,
            ticket,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(database)?;
        let Some(found) = found else {
            return Ok(None);
        };
        let done = sqlx::query!(
            "UPDATE reviews SET state = ?, decided_by = ?, decided_at = ?, version = version + 1
             WHERE id = ? AND state = 'pending'",
            state,
            by,
            at,
            found.id,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if done.rows_affected() == 0 {
            return Ok(None);
        }
        let change = change_feed::record(
            &mut tx,
            Entity::Worktree,
            found.worktree_id,
            ChangeOp::Update,
            Timestamp::from_millis(at),
        )
        .await?;
        tx.commit().await.map_err(database)?;
        Ok(Some(change))
    }

    async fn state_of(&self, ticket: Id<Ticket>) -> Result<Option<ReviewState>, AppError> {
        let ticket = ticket.as_uuid();
        let state = sqlx::query_scalar!(
            r#"SELECT r.state AS "state!: String"
               FROM reviews r JOIN worktree_tickets t ON t.worktree_id = r.worktree_id
               WHERE t.ticket_id = ? ORDER BY r.created_at DESC, r.id DESC LIMIT 1"#,
            ticket,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        state
            .map(|state| {
                state
                    .parse::<ReviewState>()
                    .map_err(|_| AppError::from(corrupt("state")))
            })
            .transpose()
    }

    async fn latest(&self, worktree: Id<Worktree>) -> Result<Option<StoredReview>, AppError> {
        let uuid = worktree.as_uuid();
        let Some(row) = sqlx::query_as!(
            ReviewRow,
            r#"SELECT id AS "id: Uuid", run_id AS "run_id: Uuid", branch, state,
                      decided_by AS "decided_by: Uuid", decided_at, created_at, version
               FROM reviews WHERE worktree_id = ? ORDER BY created_at DESC, id DESC LIMIT 1"#,
            uuid,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?
        else {
            return Ok(None);
        };
        let record = ReviewRecord {
            id: Id::from_uuid(row.id),
            worktree,
            run: row.run_id.map(Id::<Run>::from_uuid),
            branch: row.branch,
            state: row
                .state
                .parse::<ReviewState>()
                .map_err(|_| corrupt("state"))?,
            decided_by: row.decided_by.map(Id::<Actor>::from_uuid),
            decided_at: row.decided_at.map(Timestamp::from_millis),
            created_at: Timestamp::from_millis(row.created_at),
            version: Version::from_stored(row.version),
        };
        let rows = sqlx::query!(
            "SELECT command, ok, output FROM review_test_runs WHERE review_id = ?
             ORDER BY position",
            row.id,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        let tests = rows
            .into_iter()
            .map(|row| TestRun {
                command: row.command,
                ok: row.ok.map(|ok| ok != 0),
                output: row.output,
            })
            .collect();
        Ok(Some(StoredReview {
            review: Review::restore(record),
            tests,
        }))
    }
}
