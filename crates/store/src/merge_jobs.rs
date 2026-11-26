use async_trait::async_trait;
use nullradix_app::dispatch::merge_jobs::{
    MAX_OPEN_JOBS, MAX_TICKET_JOBS, MergeJobRepository, StoredMergeJob,
};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    Id, MergeJob, MergeJobRecord, MergePhase, MergeStrategy, Ticket, Timestamp, Version, Worktree,
};
use uuid::Uuid;

use crate::error::database;
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid merge job {what}"))
}

struct Row {
    id: Uuid,
    worktree_id: Uuid,
    phase: String,
    target_branch: Option<String>,
    target_head: Option<String>,
    strategy: String,
    conflict_sessions: i64,
    session_used: i64,
    conflict_seen: i64,
    wait_noted: i64,
    held_by: Option<String>,
    hold_noticed: i64,
    problem: Option<String>,
    merged_into: Option<String>,
    merged_commit: Option<String>,
    since: i64,
    finished_at: Option<i64>,
    version: i64,
}

impl Row {
    fn into_job(self) -> Result<MergeJob, StoreError> {
        Ok(MergeJob::restore(MergeJobRecord {
            id: Id::from_uuid(self.id),
            worktree: Id::<Worktree>::from_uuid(self.worktree_id),
            phase: self
                .phase
                .parse::<MergePhase>()
                .map_err(|_| corrupt("phase"))?,
            target_branch: self.target_branch,
            target_head: self.target_head,
            strategy: self
                .strategy
                .parse::<MergeStrategy>()
                .map_err(|_| corrupt("strategy"))?,
            conflict_sessions: u32::try_from(self.conflict_sessions)
                .map_err(|_| corrupt("session count"))?,
            session_used: self.session_used != 0,
            conflict_seen: self.conflict_seen != 0,
            wait_noted: self.wait_noted != 0,
            held_by: self.held_by,
            hold_noticed: self.hold_noticed != 0,
            problem: self.problem,
            merged_into: self.merged_into,
            merged_commit: self.merged_commit,
            since: Timestamp::from_millis(self.since),
            finished_at: self.finished_at.map(Timestamp::from_millis),
            version: Version::from_stored(self.version),
        }))
    }
}

#[async_trait]
impl MergeJobRepository for Store {
    async fn queue(&self, job: &MergeJob) -> Result<Option<Change>, AppError> {
        let record = job.record();
        let id = record.id.as_uuid();
        let worktree = record.worktree.as_uuid();
        let phase = record.phase.as_str();
        let strategy = record.strategy.as_str();
        let since = record.since.as_millis();
        let version = record.version.value();
        let mut tx = self.begin().await?;
        let open = sqlx::query_scalar!(
            "SELECT 1 FROM merge_jobs
             WHERE worktree_id = ? AND phase IN ('pending', 'waiting', 'conflict_session')",
            worktree
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(database)?;
        if open.is_some() {
            return Ok(None);
        }
        sqlx::query!(
            "INSERT INTO merge_jobs (id, worktree_id, phase, target_branch, strategy, since, version)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            id,
            worktree,
            phase,
            record.target_branch,
            strategy,
            since,
            version,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        let change = change_feed::record(
            &mut tx,
            Entity::Worktree,
            worktree,
            ChangeOp::Update,
            record.since,
        )
        .await?;
        tx.commit().await.map_err(database)?;
        Ok(Some(change))
    }

    async fn open(&self) -> Result<Vec<MergeJob>, AppError> {
        let limit = i64::from(MAX_OPEN_JOBS);
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", worktree_id AS "worktree_id: Uuid", phase, target_branch,
                      target_head, strategy, conflict_sessions, session_used, conflict_seen,
                      wait_noted, held_by, hold_noticed, problem, merged_into, merged_commit,
                      since, finished_at, version
               FROM merge_jobs
               WHERE phase IN ('pending', 'waiting', 'conflict_session')
               ORDER BY since, id LIMIT ?"#,
            limit,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows
            .into_iter()
            .map(Row::into_job)
            .collect::<Result<Vec<_>, _>>()?)
    }

    async fn save(
        &self,
        job: &MergeJob,
        expected: Version,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let record = job.record();
        let id = record.id.as_uuid();
        let worktree = record.worktree.as_uuid();
        let phase = record.phase.as_str();
        let strategy = record.strategy.as_str();
        let sessions = i64::from(record.conflict_sessions);
        let session_used = i64::from(record.session_used);
        let conflict_seen = i64::from(record.conflict_seen);
        let wait_noted = i64::from(record.wait_noted);
        let hold_noticed = i64::from(record.hold_noticed);
        let finished = record.finished_at.map(Timestamp::as_millis);
        let expected = expected.value();
        let mut tx = self.begin().await?;
        let updated = sqlx::query!(
            "UPDATE merge_jobs
             SET phase = ?, target_branch = ?, target_head = ?, strategy = ?,
                 conflict_sessions = ?, session_used = ?, conflict_seen = ?, wait_noted = ?,
                 held_by = ?, hold_noticed = ?, problem = ?, merged_into = ?, merged_commit = ?,
                 finished_at = ?, version = version + 1
             WHERE id = ? AND version = ?",
            phase,
            record.target_branch,
            record.target_head,
            strategy,
            sessions,
            session_used,
            conflict_seen,
            wait_noted,
            record.held_by,
            hold_noticed,
            record.problem,
            record.merged_into,
            record.merged_commit,
            finished,
            id,
            expected,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        let change =
            change_feed::record(&mut tx, Entity::Worktree, worktree, ChangeOp::Update, at).await?;
        tx.commit().await.map_err(database)?;
        Ok(Some(change))
    }

    async fn of_ticket(&self, ticket: Id<Ticket>) -> Result<Vec<StoredMergeJob>, AppError> {
        let ticket = ticket.as_uuid();
        let limit = i64::from(MAX_TICKET_JOBS);
        let rows = sqlx::query!(
            r#"SELECT j.id AS "id: Uuid", j.worktree_id AS "worktree_id: Uuid", j.phase,
                      j.target_branch, j.target_head, j.strategy, j.conflict_sessions,
                      j.session_used, j.conflict_seen, j.wait_noted, j.held_by, j.hold_noticed,
                      j.problem, j.merged_into, j.merged_commit, j.since, j.finished_at,
                      j.version, w.branch AS "branch!"
               FROM merge_jobs j
               JOIN worktrees w ON w.id = j.worktree_id
               WHERE j.worktree_id IN (SELECT worktree_id FROM worktree_tickets WHERE ticket_id = ?)
               ORDER BY j.since DESC, j.id DESC LIMIT ?"#,
            ticket,
            limit,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            let job = Row {
                id: row.id,
                worktree_id: row.worktree_id,
                phase: row.phase,
                target_branch: row.target_branch,
                target_head: row.target_head,
                strategy: row.strategy,
                conflict_sessions: row.conflict_sessions,
                session_used: row.session_used,
                conflict_seen: row.conflict_seen,
                wait_noted: row.wait_noted,
                held_by: row.held_by,
                hold_noticed: row.hold_noticed,
                problem: row.problem,
                merged_into: row.merged_into,
                merged_commit: row.merged_commit,
                since: row.since,
                finished_at: row.finished_at,
                version: row.version,
            }
            .into_job()?;
            found.push(StoredMergeJob {
                job,
                branch: row.branch,
            });
        }
        Ok(found)
    }

    async fn open_of(&self, worktree: Id<Worktree>) -> Result<Option<MergeJob>, AppError> {
        let worktree = worktree.as_uuid();
        let row = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", worktree_id AS "worktree_id: Uuid", phase, target_branch,
                      target_head, strategy, conflict_sessions, session_used, conflict_seen,
                      wait_noted, held_by, hold_noticed, problem, merged_into, merged_commit,
                      since, finished_at, version
               FROM merge_jobs
               WHERE worktree_id = ? AND phase IN ('pending', 'waiting', 'conflict_session')"#,
            worktree,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(row.map(Row::into_job).transpose()?)
    }
}
