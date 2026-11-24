use async_trait::async_trait;
use nullradix_app::repositories::{
    CheckoutRepository, RepositorySummary, RunnerRepository, checkout_exists,
};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    Checkout, CheckoutPath, CheckoutRecord, Id, Repository, RepositoryName, RepositoryRecord,
    Runner, RunnerName, RunnerOs, RunnerRecord, Timestamp, Version,
};
use uuid::Uuid;

use crate::error::{database, unique_violation};
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid {what}"))
}

struct RunnerRow {
    id: Uuid,
    name: String,
    os: String,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

impl RunnerRow {
    fn into_runner(self) -> Result<Runner, StoreError> {
        Ok(Runner::restore(RunnerRecord {
            id: Id::from_uuid(self.id),
            name: RunnerName::parse(&self.name).map_err(|_| corrupt("runner name"))?,
            os: self
                .os
                .parse::<RunnerOs>()
                .map_err(|_| corrupt("runner os"))?,
            created_at: Timestamp::from_millis(self.created_at),
            updated_at: Timestamp::from_millis(self.updated_at),
            version: Version::from_stored(self.version),
        }))
    }
}

#[async_trait]
impl RunnerRepository for Store {
    async fn ensure(&self, candidate: &Runner) -> Result<(Runner, Option<Change>), AppError> {
        let id = candidate.id().as_uuid();
        let name = candidate.name().as_str();
        let os = candidate.os().as_str();
        let created_at = candidate.created_at().as_millis();
        let updated_at = candidate.updated_at().as_millis();
        let version = candidate.version().value();
        let mut tx = self.begin().await?;
        let inserted = sqlx::query!(
            "INSERT INTO runners (id, name, os, created_at, updated_at, version)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (name) DO NOTHING",
            id,
            name,
            os,
            created_at,
            updated_at,
            version,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        let change = if inserted.rows_affected() == 0 {
            None
        } else {
            Some(
                change_feed::record(
                    &mut tx,
                    Entity::Runner,
                    id,
                    ChangeOp::Insert,
                    candidate.created_at(),
                )
                .await?,
            )
        };
        let row = sqlx::query_as!(
            RunnerRow,
            r#"SELECT id AS "id: Uuid", name, os, created_at, updated_at, version
               FROM runners WHERE name = ?"#,
            name,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok((row.into_runner()?, change))
    }

    async fn get(&self, id: Id<Runner>) -> Result<Option<Runner>, AppError> {
        let uuid = id.as_uuid();
        let row = sqlx::query_as!(
            RunnerRow,
            r#"SELECT id AS "id: Uuid", name, os, created_at, updated_at, version
               FROM runners WHERE id = ?"#,
            uuid,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(row.map(RunnerRow::into_runner).transpose()?)
    }

    async fn list(&self) -> Result<Vec<Runner>, AppError> {
        let rows = sqlx::query_as!(
            RunnerRow,
            r#"SELECT id AS "id: Uuid", name, os, created_at, updated_at, version
               FROM runners ORDER BY name, id"#,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows
            .into_iter()
            .map(RunnerRow::into_runner)
            .collect::<Result<_, _>>()?)
    }
}

/// A checkout joined with its repository.
struct CheckoutRow {
    id: Uuid,
    repository_id: Uuid,
    repository_name: String,
    repository_created_at: i64,
    repository_updated_at: i64,
    repository_version: i64,
    runner_id: Uuid,
    path: String,
    path_key: String,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

impl CheckoutRow {
    fn into_pair(self) -> Result<(Repository, Checkout), StoreError> {
        let repository = Repository::restore(RepositoryRecord {
            id: Id::from_uuid(self.repository_id),
            name: RepositoryName::parse(&self.repository_name)
                .map_err(|_| corrupt("repository name"))?,
            created_at: Timestamp::from_millis(self.repository_created_at),
            updated_at: Timestamp::from_millis(self.repository_updated_at),
            version: Version::from_stored(self.repository_version),
        });
        let checkout = Checkout::restore(CheckoutRecord {
            id: Id::from_uuid(self.id),
            repository: repository.id(),
            runner: Id::from_uuid(self.runner_id),
            path: CheckoutPath::parse(&self.path).map_err(|_| corrupt("checkout path"))?,
            key: CheckoutPath::parse(&self.path_key).map_err(|_| corrupt("checkout key"))?,
            created_at: Timestamp::from_millis(self.created_at),
            updated_at: Timestamp::from_millis(self.updated_at),
            version: Version::from_stored(self.version),
        });
        Ok((repository, checkout))
    }
}

struct RepositoryRow {
    id: Uuid,
    name: String,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

impl RepositoryRow {
    fn into_summary(self) -> Result<RepositorySummary, StoreError> {
        Ok(RepositorySummary {
            repository: Repository::restore(RepositoryRecord {
                id: Id::from_uuid(self.id),
                name: RepositoryName::parse(&self.name).map_err(|_| corrupt("repository name"))?,
                created_at: Timestamp::from_millis(self.created_at),
                updated_at: Timestamp::from_millis(self.updated_at),
                version: Version::from_stored(self.version),
            }),
            checkouts: Vec::new(),
        })
    }
}

#[async_trait]
impl CheckoutRepository for Store {
    async fn find(
        &self,
        runner: Id<Runner>,
        key: &CheckoutPath,
    ) -> Result<Option<(Repository, Checkout)>, AppError> {
        let runner = runner.as_uuid();
        let key = key.as_str();
        let row = sqlx::query_as!(
            CheckoutRow,
            r#"SELECT c.id AS "id: Uuid", c.repository_id AS "repository_id: Uuid",
                      r.name AS repository_name, r.created_at AS repository_created_at,
                      r.updated_at AS repository_updated_at, r.version AS repository_version,
                      c.runner_id AS "runner_id: Uuid", c.path, c.path_key, c.created_at,
                      c.updated_at, c.version
               FROM checkouts c
               JOIN repositories r ON r.id = c.repository_id
               WHERE c.runner_id = ? AND c.path_key = ?"#,
            runner,
            key,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(row.map(CheckoutRow::into_pair).transpose()?)
    }

    async fn insert(
        &self,
        repository: &Repository,
        checkout: &Checkout,
    ) -> Result<Change, AppError> {
        let repository_id = repository.id().as_uuid();
        let name = repository.name().as_str();
        let created_at = repository.created_at().as_millis();
        let updated_at = repository.updated_at().as_millis();
        let version = repository.version().value();
        let mut tx = self.begin().await?;
        sqlx::query!(
            "INSERT INTO repositories (id, name, created_at, updated_at, version)
             VALUES (?, ?, ?, ?, ?)",
            repository_id,
            name,
            created_at,
            updated_at,
            version,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        let id = checkout.id().as_uuid();
        let checkout_repository = checkout.repository().as_uuid();
        let runner = checkout.runner().as_uuid();
        let path = checkout.path().as_str();
        let key = checkout.key().as_str();
        let checkout_created_at = checkout.created_at().as_millis();
        let checkout_updated_at = checkout.updated_at().as_millis();
        let checkout_version = checkout.version().value();
        let inserted = sqlx::query!(
            "INSERT INTO checkouts (id, repository_id, runner_id, path, path_key, created_at,
                                    updated_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            checkout_repository,
            runner,
            path,
            key,
            checkout_created_at,
            checkout_updated_at,
            checkout_version,
        )
        .execute(&mut *tx)
        .await;
        match inserted {
            Ok(_) => {}
            Err(error) if unique_violation(&error, "checkouts.path_key") => {
                return Err(checkout_exists(key));
            }
            Err(error) => return Err(database(error)),
        }
        let change = change_feed::record(
            &mut tx,
            Entity::Repository,
            repository_id,
            ChangeOp::Insert,
            repository.created_at(),
        )
        .await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn list(&self) -> Result<Vec<RepositorySummary>, AppError> {
        let rows = sqlx::query_as!(
            RepositoryRow,
            r#"SELECT id AS "id: Uuid", name, created_at, updated_at, version
               FROM repositories ORDER BY name COLLATE NOCASE, id"#,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        let checkouts = sqlx::query_as!(
            CheckoutRow,
            r#"SELECT c.id AS "id: Uuid", c.repository_id AS "repository_id: Uuid",
                      r.name AS repository_name, r.created_at AS repository_created_at,
                      r.updated_at AS repository_updated_at, r.version AS repository_version,
                      c.runner_id AS "runner_id: Uuid", c.path, c.path_key, c.created_at,
                      c.updated_at, c.version
               FROM checkouts c
               JOIN repositories r ON r.id = c.repository_id
               ORDER BY c.path_key, c.id"#,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        let mut summaries = rows
            .into_iter()
            .map(RepositoryRow::into_summary)
            .collect::<Result<Vec<_>, _>>()?;
        for row in checkouts {
            let (repository, checkout) = row.into_pair()?;
            if let Some(summary) = summaries
                .iter_mut()
                .find(|summary| summary.repository.id() == repository.id())
            {
                summary.checkouts.push(checkout);
            }
        }
        Ok(summaries)
    }
}
