use async_trait::async_trait;
use nullradix_app::projects::{self, ProjectRepository};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{Id, Project, ProjectKey, ProjectName, Timestamp, Version};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::error::{database, missed_guard, unique_violation};
use crate::{Store, StoreError, change_feed};

struct Row {
    id: Uuid,
    key: String,
    name: String,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

impl Row {
    fn into_project(self) -> Result<Project, StoreError> {
        let key = ProjectKey::parse(&self.key)
            .map_err(|_| StoreError::Corrupt(format!("invalid project key `{}`", self.key)))?;
        let name = ProjectName::parse(&self.name)
            .map_err(|_| StoreError::Corrupt(String::from("invalid project name")))?;
        Ok(Project::restore(
            Id::from_uuid(self.id),
            key,
            name,
            Timestamp::from_millis(self.created_at),
            Timestamp::from_millis(self.updated_at),
            Version::from_stored(self.version),
        ))
    }
}

async fn missed(connection: &mut SqliteConnection, id: Id<Project>) -> AppError {
    let uuid = id.as_uuid();
    let exists = sqlx::query_scalar!("SELECT 1 FROM projects WHERE id = ?", uuid)
        .fetch_optional(connection)
        .await;
    missed_guard(
        exists,
        AppError::version_conflict("project", id),
        projects::not_found(id),
    )
}

#[async_trait]
impl ProjectRepository for Store {
    async fn insert(&self, project: &Project) -> Result<Change, AppError> {
        let id = project.id().as_uuid();
        let key = project.key().as_str();
        let name = project.name().as_str();
        let created_at = project.created_at().as_millis();
        let updated_at = project.updated_at().as_millis();
        let version = project.version().value();
        let mut tx = self.begin().await?;
        let inserted = sqlx::query!(
            "INSERT INTO projects (id, key, name, created_at, updated_at, version)
             VALUES (?, ?, ?, ?, ?, ?)",
            id,
            key,
            name,
            created_at,
            updated_at,
            version,
        )
        .execute(&mut *tx)
        .await;
        match inserted {
            Err(error) if unique_violation(&error, "projects.key") => {
                return Err(projects::key_taken(project.key()));
            }
            other => other.map_err(database)?,
        };
        let change = change_feed::record(
            &mut tx,
            Entity::Project,
            id,
            ChangeOp::Insert,
            project.created_at(),
        )
        .await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn list(&self) -> Result<Vec<Project>, AppError> {
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", key, name, created_at, updated_at, version
               FROM projects ORDER BY key"#
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows
            .into_iter()
            .map(Row::into_project)
            .collect::<Result<_, _>>()?)
    }

    async fn get(&self, id: Id<Project>) -> Result<Option<Project>, AppError> {
        let uuid = id.as_uuid();
        let row = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", key, name, created_at, updated_at, version
               FROM projects WHERE id = ?"#,
            uuid,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(row.map(Row::into_project).transpose()?)
    }

    async fn update(&self, project: &Project, expected: Version) -> Result<Change, AppError> {
        let uuid = project.id().as_uuid();
        let name = project.name().as_str();
        let updated_at = project.updated_at().as_millis();
        let version = project.version().value();
        let expected = expected.value();
        let mut tx = self.begin().await?;
        let updated = sqlx::query!(
            "UPDATE projects SET name = ?, updated_at = ?, version = ?
             WHERE id = ? AND version = ?",
            name,
            updated_at,
            version,
            uuid,
            expected,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if updated.rows_affected() == 0 {
            return Err(missed(&mut tx, project.id()).await);
        }
        let change = change_feed::record(
            &mut tx,
            Entity::Project,
            uuid,
            ChangeOp::Update,
            project.updated_at(),
        )
        .await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn delete(
        &self,
        id: Id<Project>,
        expected: Version,
        at: Timestamp,
    ) -> Result<Change, AppError> {
        let uuid = id.as_uuid();
        let expected = expected.value();
        let mut tx = self.begin().await?;
        let deleted = sqlx::query!(
            "DELETE FROM projects WHERE id = ? AND version = ?",
            uuid,
            expected,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if deleted.rows_affected() == 0 {
            return Err(missed(&mut tx, id).await);
        }
        let change =
            change_feed::record(&mut tx, Entity::Project, uuid, ChangeOp::Delete, at).await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }
}
