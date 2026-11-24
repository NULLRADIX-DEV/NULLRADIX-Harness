use async_trait::async_trait;
use nullradix_app::projects::ProjectRepository;
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{Id, Project, ProjectKey, ProjectName, Timestamp, Version};
use sqlx::SqliteConnection;
use uuid::Uuid;

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
        Ok(Project {
            id: Id::from_uuid(self.id),
            key,
            name,
            created_at: Timestamp::from_millis(self.created_at),
            updated_at: Timestamp::from_millis(self.updated_at),
            version: Version::from_stored(self.version),
        })
    }
}

fn database(error: sqlx::Error) -> AppError {
    StoreError::from(error).into()
}

fn version_conflict(id: Id<Project>) -> AppError {
    AppError::conflict(
        "version-conflict",
        format!("project {id} was changed by someone else, reload and try again"),
    )
}

/// Tells a missing project apart from a stale version after a guarded update matched nothing.
async fn missed_update(connection: &mut SqliteConnection, id: Id<Project>) -> AppError {
    let uuid = id.as_uuid();
    match sqlx::query_scalar!("SELECT 1 FROM projects WHERE id = ?", uuid)
        .fetch_optional(connection)
        .await
    {
        Ok(Some(_)) => version_conflict(id),
        Ok(None) => nullradix_app::projects::not_found(id),
        Err(error) => database(error),
    }
}

#[async_trait]
impl ProjectRepository for Store {
    async fn insert(&self, project: &Project) -> Result<Change, AppError> {
        let id = project.id.as_uuid();
        let key = project.key.as_str();
        let name = project.name.as_str();
        let created_at = project.created_at.as_millis();
        let updated_at = project.updated_at.as_millis();
        let version = project.version.value();
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
        if let Err(sqlx::Error::Database(error)) = &inserted
            && error.is_unique_violation()
        {
            return Err(AppError::conflict(
                "project-key-taken",
                format!("another project already uses the key {key}"),
            ));
        }
        inserted.map_err(database)?;
        let change = change_feed::record(
            &mut tx,
            Entity::Project,
            id,
            ChangeOp::Insert,
            project.created_at,
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

    async fn rename(
        &self,
        id: Id<Project>,
        name: &ProjectName,
        expected: Version,
        at: Timestamp,
    ) -> Result<(Project, Change), AppError> {
        let uuid = id.as_uuid();
        let name = name.as_str();
        let millis = at.as_millis();
        let expected = expected.value();
        let mut tx = self.begin().await?;
        let row = sqlx::query_as!(
            Row,
            r#"UPDATE projects SET name = ?, updated_at = ?, version = version + 1
               WHERE id = ? AND version = ?
               RETURNING id AS "id: Uuid", key, name, created_at, updated_at, version"#,
            name,
            millis,
            uuid,
            expected,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(database)?;
        let Some(row) = row else {
            return Err(missed_update(&mut tx, id).await);
        };
        let project = row.into_project()?;
        let change =
            change_feed::record(&mut tx, Entity::Project, uuid, ChangeOp::Update, at).await?;
        tx.commit().await.map_err(database)?;
        Ok((project, change))
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
            return Err(missed_update(&mut tx, id).await);
        }
        let change =
            change_feed::record(&mut tx, Entity::Project, uuid, ChangeOp::Delete, at).await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }
}
