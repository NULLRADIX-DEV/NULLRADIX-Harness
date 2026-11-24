use async_trait::async_trait;
use nullradix_app::runs::{self, RunRepository};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{Id, Run, RunLabel, RunRecord, RunStatus, Timestamp, Version};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::error::{database, missed_guard};
use crate::{Store, StoreError, change_feed};

struct Row {
    id: Uuid,
    label: String,
    status: String,
    pid: Option<i64>,
    exit_code: Option<i64>,
    error: Option<String>,
    created_at: i64,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    version: i64,
}

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid run {what}"))
}

impl Row {
    fn into_run(self) -> Result<Run, StoreError> {
        let label = RunLabel::parse(&self.label).map_err(|_| corrupt("label"))?;
        let status = self
            .status
            .parse::<RunStatus>()
            .map_err(|_| corrupt("status"))?;
        let pid = self
            .pid
            .map(u32::try_from)
            .transpose()
            .map_err(|_| corrupt("pid"))?;
        let exit_code = self
            .exit_code
            .map(i32::try_from)
            .transpose()
            .map_err(|_| corrupt("exit code"))?;
        Ok(Run::restore(RunRecord {
            id: Id::from_uuid(self.id),
            label,
            status,
            pid,
            exit_code,
            error: self.error,
            created_at: Timestamp::from_millis(self.created_at),
            started_at: self.started_at.map(Timestamp::from_millis),
            finished_at: self.finished_at.map(Timestamp::from_millis),
            version: Version::from_stored(self.version),
        }))
    }
}

async fn missed(connection: &mut SqliteConnection, id: Id<Run>) -> AppError {
    let uuid = id.as_uuid();
    let exists = sqlx::query_scalar!("SELECT 1 FROM runs WHERE id = ?", uuid)
        .fetch_optional(connection)
        .await;
    missed_guard(
        exists,
        AppError::version_conflict("run", id),
        runs::not_found(id),
    )
}

fn changed_at(run: &Run) -> Timestamp {
    run.finished_at()
        .or(run.started_at())
        .unwrap_or_else(|| run.created_at())
}

#[async_trait]
impl RunRepository for Store {
    async fn insert(&self, run: &Run) -> Result<Change, AppError> {
        let id = run.id().as_uuid();
        let label = run.label().as_str();
        let status = run.status().as_str();
        let pid = run.pid().map(i64::from);
        let exit_code = run.exit_code().map(i64::from);
        let error = run.error();
        let created_at = run.created_at().as_millis();
        let started_at = run.started_at().map(Timestamp::as_millis);
        let finished_at = run.finished_at().map(Timestamp::as_millis);
        let version = run.version().value();
        let mut tx = self.begin().await?;
        sqlx::query!(
            "INSERT INTO runs (id, label, status, pid, exit_code, error, created_at, started_at,
                               finished_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            label,
            status,
            pid,
            exit_code,
            error,
            created_at,
            started_at,
            finished_at,
            version,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        let change =
            change_feed::record(&mut tx, Entity::Run, id, ChangeOp::Insert, run.created_at())
                .await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn update(&self, run: &Run, expected: Version) -> Result<Change, AppError> {
        let id = run.id().as_uuid();
        let status = run.status().as_str();
        let pid = run.pid().map(i64::from);
        let exit_code = run.exit_code().map(i64::from);
        let error = run.error();
        let started_at = run.started_at().map(Timestamp::as_millis);
        let finished_at = run.finished_at().map(Timestamp::as_millis);
        let version = run.version().value();
        let expected = expected.value();
        let mut tx = self.begin().await?;
        let updated = sqlx::query!(
            "UPDATE runs SET status = ?, pid = ?, exit_code = ?, error = ?, started_at = ?,
                             finished_at = ?, version = ?
             WHERE id = ? AND version = ?",
            status,
            pid,
            exit_code,
            error,
            started_at,
            finished_at,
            version,
            id,
            expected,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if updated.rows_affected() == 0 {
            return Err(missed(&mut tx, run.id()).await);
        }
        let change =
            change_feed::record(&mut tx, Entity::Run, id, ChangeOp::Update, changed_at(run))
                .await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn get(&self, id: Id<Run>) -> Result<Option<Run>, AppError> {
        let uuid = id.as_uuid();
        let row = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", label, status, pid, exit_code, error, created_at,
                      started_at, finished_at, version
               FROM runs WHERE id = ?"#,
            uuid,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(row.map(Row::into_run).transpose()?)
    }

    async fn list(&self, limit: u32) -> Result<Vec<Run>, AppError> {
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", label, status, pid, exit_code, error, created_at,
                      started_at, finished_at, version
               FROM runs ORDER BY created_at DESC, id DESC LIMIT ?"#,
            limit,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows
            .into_iter()
            .map(Row::into_run)
            .collect::<Result<_, _>>()?)
    }

    async fn active(&self) -> Result<Vec<Run>, AppError> {
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", label, status, pid, exit_code, error, created_at,
                      started_at, finished_at, version
               FROM runs WHERE status IN ('starting', 'running') ORDER BY created_at"#,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows
            .into_iter()
            .map(Row::into_run)
            .collect::<Result<_, _>>()?)
    }
}
