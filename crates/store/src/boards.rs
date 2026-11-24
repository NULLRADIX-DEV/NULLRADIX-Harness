use async_trait::async_trait;
use nullradix_app::boards::{self, ActorRepository, BoardRepository, BoardSummary};
use nullradix_app::{AppError, Change, ChangeOp, Entity, projects};
use nullradix_domain::{
    Actor, ActorKind, ActorName, Board, BoardName, BoardRecord, CheckoutPath, Id, Project, Runner,
    Timestamp, Version,
};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::error::{database, foreign_key_violation, missed_guard};
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid {what}"))
}

struct ActorRow {
    id: Uuid,
    kind: String,
    name: String,
    created_at: i64,
}

impl ActorRow {
    fn into_actor(self) -> Result<Actor, StoreError> {
        let kind = self
            .kind
            .parse::<ActorKind>()
            .map_err(|_| corrupt("actor kind"))?;
        let name = ActorName::parse(&self.name).map_err(|_| corrupt("actor name"))?;
        Ok(Actor::new(
            Id::from_uuid(self.id),
            kind,
            name,
            Timestamp::from_millis(self.created_at),
        ))
    }
}

#[async_trait]
impl ActorRepository for Store {
    async fn ensure(&self, candidate: &Actor) -> Result<Actor, AppError> {
        let id = candidate.id().as_uuid();
        let kind = candidate.kind().as_str();
        let name = candidate.name().as_str();
        let created_at = candidate.created_at().as_millis();
        let mut tx = self.begin().await?;
        sqlx::query!(
            "INSERT INTO actors (id, kind, name, created_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (kind, name) DO NOTHING",
            id,
            kind,
            name,
            created_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        let row = sqlx::query_as!(
            ActorRow,
            r#"SELECT id AS "id: Uuid", kind, name, created_at FROM actors
               WHERE kind = ? AND name = ?"#,
            kind,
            name,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(row.into_actor()?)
    }
}

struct Row {
    id: Uuid,
    project_id: Uuid,
    project_key: String,
    name: String,
    owner_id: Uuid,
    owner_name: String,
    dispatch_enabled: i64,
    repository_id: Option<Uuid>,
    repository_name: Option<String>,
    open_tickets: i64,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

impl Row {
    fn into_summary(self) -> Result<BoardSummary, StoreError> {
        let name = BoardName::parse(&self.name).map_err(|_| corrupt("board name"))?;
        let board = Board::restore(BoardRecord {
            id: Id::from_uuid(self.id),
            project: Id::from_uuid(self.project_id),
            name,
            owner: Id::from_uuid(self.owner_id),
            dispatch_enabled: self.dispatch_enabled != 0,
            repository: self.repository_id.map(Id::from_uuid),
            created_at: Timestamp::from_millis(self.created_at),
            updated_at: Timestamp::from_millis(self.updated_at),
            version: Version::from_stored(self.version),
        });
        Ok(BoardSummary {
            board,
            project_key: self.project_key,
            owner_name: self.owner_name,
            repository_name: self.repository_name,
            open_tickets: u32::try_from(self.open_tickets).map_err(|_| corrupt("ticket count"))?,
        })
    }
}

pub(crate) async fn missed(connection: &mut SqliteConnection, id: Id<Board>) -> AppError {
    let uuid = id.as_uuid();
    let exists = sqlx::query_scalar!("SELECT 1 FROM boards WHERE id = ?", uuid)
        .fetch_optional(connection)
        .await;
    missed_guard(
        exists,
        AppError::version_conflict("board", id),
        boards::not_found(id),
    )
}

/// Names the missing project when it caused `error`; a missing owner stays a database error.
async fn missing_project(
    connection: &mut SqliteConnection,
    project: Id<Project>,
    error: sqlx::Error,
) -> AppError {
    let uuid = project.as_uuid();
    let exists = sqlx::query_scalar!("SELECT 1 FROM projects WHERE id = ?", uuid)
        .fetch_optional(connection)
        .await;
    missed_guard(exists, database(error), projects::not_found(project))
}

fn summaries(rows: Vec<Row>) -> Result<Vec<BoardSummary>, AppError> {
    Ok(rows
        .into_iter()
        .map(Row::into_summary)
        .collect::<Result<_, _>>()?)
}

#[async_trait]
impl BoardRepository for Store {
    async fn insert(&self, board: &Board) -> Result<Change, AppError> {
        let id = board.id().as_uuid();
        let project = board.project().as_uuid();
        let name = board.name().as_str();
        let owner = board.owner().as_uuid();
        let dispatch = i64::from(board.dispatch_enabled());
        let created_at = board.created_at().as_millis();
        let updated_at = board.updated_at().as_millis();
        let version = board.version().value();
        let mut tx = self.begin().await?;
        let inserted = sqlx::query!(
            "INSERT INTO boards (id, project_id, name, owner_id, dispatch_enabled, created_at,
                                 updated_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            project,
            name,
            owner,
            dispatch,
            created_at,
            updated_at,
            version,
        )
        .execute(&mut *tx)
        .await;
        match inserted {
            Ok(_) => {}
            Err(error) if foreign_key_violation(&error) => {
                return Err(missing_project(&mut tx, board.project(), error).await);
            }
            Err(error) => return Err(database(error)),
        }
        let change = change_feed::record(
            &mut tx,
            Entity::Board,
            id,
            ChangeOp::Insert,
            board.created_at(),
        )
        .await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn list(&self, project: Option<Id<Project>>) -> Result<Vec<BoardSummary>, AppError> {
        let project = project.map(|id| id.as_uuid());
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT b.id AS "id: Uuid", b.project_id AS "project_id: Uuid", p.key AS project_key,
                      b.name, b.owner_id AS "owner_id: Uuid", a.name AS owner_name,
                      b.dispatch_enabled, b.repository_id AS "repository_id?: Uuid",
                      r.name AS "repository_name?",
                      (SELECT count(*) FROM tickets t
                       WHERE t.board_id = b.id AND t.status NOT IN ('done', 'not-needed'))
                        AS "open_tickets!: i64",
                      b.created_at, b.updated_at, b.version
               FROM boards b
               JOIN projects p ON p.id = b.project_id
               JOIN actors a ON a.id = b.owner_id
               LEFT JOIN repositories r ON r.id = b.repository_id
               WHERE ?1 IS NULL OR b.project_id = ?1
               ORDER BY p.key, b.name COLLATE NOCASE, b.id"#,
            project,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        summaries(rows)
    }

    async fn get(&self, id: Id<Board>) -> Result<Option<BoardSummary>, AppError> {
        let uuid = id.as_uuid();
        let row = sqlx::query_as!(
            Row,
            r#"SELECT b.id AS "id: Uuid", b.project_id AS "project_id: Uuid", p.key AS project_key,
                      b.name, b.owner_id AS "owner_id: Uuid", a.name AS owner_name,
                      b.dispatch_enabled, b.repository_id AS "repository_id?: Uuid",
                      r.name AS "repository_name?",
                      (SELECT count(*) FROM tickets t
                       WHERE t.board_id = b.id AND t.status NOT IN ('done', 'not-needed'))
                        AS "open_tickets!: i64",
                      b.created_at, b.updated_at, b.version
               FROM boards b
               JOIN projects p ON p.id = b.project_id
               JOIN actors a ON a.id = b.owner_id
               LEFT JOIN repositories r ON r.id = b.repository_id
               WHERE b.id = ?"#,
            uuid,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(row.map(Row::into_summary).transpose()?)
    }

    async fn update(&self, board: &Board, expected: Version) -> Result<Change, AppError> {
        let id = board.id().as_uuid();
        let name = board.name().as_str();
        let dispatch = i64::from(board.dispatch_enabled());
        let repository = board.repository().map(|id| id.as_uuid());
        let updated_at = board.updated_at().as_millis();
        let version = board.version().value();
        let expected = expected.value();
        let mut tx = self.begin().await?;
        let updated = sqlx::query!(
            "UPDATE boards SET name = ?, dispatch_enabled = ?, repository_id = ?, updated_at = ?,
                               version = ?
             WHERE id = ? AND version = ?",
            name,
            dispatch,
            repository,
            updated_at,
            version,
            id,
            expected,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if updated.rows_affected() == 0 {
            return Err(missed(&mut tx, board.id()).await);
        }
        let change = change_feed::record(
            &mut tx,
            Entity::Board,
            id,
            ChangeOp::Update,
            board.updated_at(),
        )
        .await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn list_by_checkout(
        &self,
        runner: Id<Runner>,
        key: &CheckoutPath,
    ) -> Result<Vec<BoardSummary>, AppError> {
        let runner = runner.as_uuid();
        let key = key.as_str();
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT b.id AS "id: Uuid", b.project_id AS "project_id: Uuid", p.key AS project_key,
                      b.name, b.owner_id AS "owner_id: Uuid", a.name AS owner_name,
                      b.dispatch_enabled, b.repository_id AS "repository_id?: Uuid",
                      r.name AS "repository_name?",
                      (SELECT count(*) FROM tickets t
                       WHERE t.board_id = b.id AND t.status NOT IN ('done', 'not-needed'))
                        AS "open_tickets!: i64",
                      b.created_at, b.updated_at, b.version
               FROM boards b
               JOIN projects p ON p.id = b.project_id
               JOIN actors a ON a.id = b.owner_id
               JOIN repositories r ON r.id = b.repository_id
               JOIN checkouts c ON c.repository_id = r.id
               WHERE c.runner_id = ? AND c.path_key = ?
               ORDER BY p.key, b.name COLLATE NOCASE, b.id"#,
            runner,
            key,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        summaries(rows)
    }
}
