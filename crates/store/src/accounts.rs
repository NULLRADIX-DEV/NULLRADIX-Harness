use async_trait::async_trait;
use nullradix_app::accounts::{self, AccountRepository};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    Account, AccountName, AccountSlug, ConfigDir, Id, Limit, Timestamp, Version,
};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::error::{database, missed_guard, unique_violation};
use crate::{Store, StoreError, change_feed};

struct Row {
    id: Uuid,
    slug: String,
    name: String,
    config_dir: Option<String>,
    enabled: i64,
    is_fallback: i64,
    position: i64,
    limit_five_hour_pct: Option<i64>,
    limit_week_pct: Option<i64>,
    token_secret_ref: Option<String>,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

fn limit(value: Option<i64>, window: &'static str) -> Result<Option<Limit>, StoreError> {
    value
        .map(|percent| {
            Limit::parse(percent, window)
                .map_err(|_| StoreError::Corrupt(format!("invalid {window} limit {percent}")))
        })
        .transpose()
}

impl Row {
    fn into_account(self) -> Result<Account, StoreError> {
        let slug = AccountSlug::parse(&self.slug)
            .map_err(|_| StoreError::Corrupt(format!("invalid account id `{}`", self.slug)))?;
        let name = AccountName::parse(&self.name)
            .map_err(|_| StoreError::Corrupt(String::from("invalid account name")))?;
        Ok(Account::restore(
            Id::from_uuid(self.id),
            slug,
            name,
            self.config_dir.map(ConfigDir::from_stored),
            self.enabled != 0,
            self.is_fallback != 0,
            self.position,
            (
                limit(self.limit_five_hour_pct, "5 hours")?,
                limit(self.limit_week_pct, "the week")?,
            ),
            self.token_secret_ref,
            (
                Timestamp::from_millis(self.created_at),
                Timestamp::from_millis(self.updated_at),
            ),
            Version::from_stored(self.version),
        ))
    }
}

async fn missed(connection: &mut SqliteConnection, id: Id<Account>) -> AppError {
    let uuid = id.as_uuid();
    let exists = sqlx::query_scalar!("SELECT 1 FROM claude_accounts WHERE id = ?", uuid)
        .fetch_optional(connection)
        .await;
    missed_guard(
        exists,
        AppError::version_conflict("account", id),
        accounts::not_found(id),
    )
}

#[async_trait]
impl AccountRepository for Store {
    async fn insert(&self, account: &Account) -> Result<Change, AppError> {
        let id = account.id().as_uuid();
        let slug = account.slug().as_str();
        let name = account.name().as_str();
        let config_dir = account.config_dir().map(ConfigDir::as_str);
        let enabled = i64::from(account.enabled());
        let fallback = i64::from(account.fallback());
        let limit_five_hour = account.limit_five_hour().map(|l| i64::from(l.percent()));
        let limit_week = account.limit_week().map(|l| i64::from(l.percent()));
        let created_at = account.created_at().as_millis();
        let updated_at = account.updated_at().as_millis();
        let version = account.version().value();
        let mut tx = self.begin().await?;
        let inserted = sqlx::query!(
            "INSERT INTO claude_accounts
               (id, slug, name, config_dir, enabled, is_fallback, position,
                limit_five_hour_pct, limit_week_pct, token_secret_ref,
                created_at, updated_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            slug,
            name,
            config_dir,
            enabled,
            fallback,
            account.position(),
            limit_five_hour,
            limit_week,
            account.token_ref(),
            created_at,
            updated_at,
            version,
        )
        .execute(&mut *tx)
        .await;
        match inserted {
            Err(error) if unique_violation(&error, "claude_accounts.slug") => {
                return Err(accounts::slug_taken(account.slug()));
            }
            other => other.map_err(database)?,
        };
        let change = change_feed::record(
            &mut tx,
            Entity::Account,
            id,
            ChangeOp::Insert,
            account.created_at(),
        )
        .await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn list(&self) -> Result<Vec<Account>, AppError> {
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", slug, name, config_dir, enabled, is_fallback, position,
                      limit_five_hour_pct, limit_week_pct, token_secret_ref,
                      created_at, updated_at, version
               FROM claude_accounts ORDER BY position, created_at, slug"#
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows
            .into_iter()
            .map(Row::into_account)
            .collect::<Result<_, _>>()?)
    }

    async fn get(&self, id: Id<Account>) -> Result<Option<Account>, AppError> {
        let uuid = id.as_uuid();
        let row = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", slug, name, config_dir, enabled, is_fallback, position,
                      limit_five_hour_pct, limit_week_pct, token_secret_ref,
                      created_at, updated_at, version
               FROM claude_accounts WHERE id = ?"#,
            uuid,
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(row.map(Row::into_account).transpose()?)
    }

    async fn update(&self, account: &Account, expected: Version) -> Result<Change, AppError> {
        let uuid = account.id().as_uuid();
        let name = account.name().as_str();
        let enabled = i64::from(account.enabled());
        let fallback = i64::from(account.fallback());
        let limit_five_hour = account.limit_five_hour().map(|l| i64::from(l.percent()));
        let limit_week = account.limit_week().map(|l| i64::from(l.percent()));
        let updated_at = account.updated_at().as_millis();
        let version = account.version().value();
        let expected = expected.value();
        let mut tx = self.begin().await?;
        let updated = sqlx::query!(
            "UPDATE claude_accounts
             SET name = ?, enabled = ?, is_fallback = ?, limit_five_hour_pct = ?,
                 limit_week_pct = ?, token_secret_ref = ?, updated_at = ?, version = ?
             WHERE id = ? AND version = ?",
            name,
            enabled,
            fallback,
            limit_five_hour,
            limit_week,
            account.token_ref(),
            updated_at,
            version,
            uuid,
            expected,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if updated.rows_affected() == 0 {
            return Err(missed(&mut tx, account.id()).await);
        }
        let change = change_feed::record(
            &mut tx,
            Entity::Account,
            uuid,
            ChangeOp::Update,
            account.updated_at(),
        )
        .await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn delete(
        &self,
        id: Id<Account>,
        expected: Version,
        at: Timestamp,
    ) -> Result<Change, AppError> {
        let uuid = id.as_uuid();
        let expected = expected.value();
        let mut tx = self.begin().await?;
        let deleted = sqlx::query!(
            "DELETE FROM claude_accounts WHERE id = ? AND version = ?",
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
            change_feed::record(&mut tx, Entity::Account, uuid, ChangeOp::Delete, at).await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn refused(sql: &str) -> bool {
        let store = Store::in_memory().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&store.writer)
            .await
            .is_err()
    }

    const NEW: &str =
        "INSERT INTO claude_accounts (id, slug, name, position, created_at, updated_at";

    #[tokio::test]
    async fn the_default_row_is_seeded_without_a_folder() {
        let store = Store::in_memory().await.unwrap();
        let row: (String, Option<String>, i64, i64) =
            sqlx::query_as("SELECT slug, config_dir, enabled, created_at FROM claude_accounts")
                .fetch_one(&store.writer)
                .await
                .unwrap();
        assert_eq!((row.0.as_str(), row.1, row.2), ("default", None, 1));
        assert!(row.3 > 1_767_225_600_000);
    }

    #[tokio::test]
    async fn the_schema_guards_slugs_limits_folders_and_references() {
        let id = "x'11111111111111111111111111111111'";
        assert!(refused(&format!("{NEW}) VALUES ({id}, 'Max', 'Max', 1, 1, 1)")).await);
        assert!(refused(&format!("{NEW}) VALUES ({id}, '-max', 'Max', 1, 1, 1)")).await);
        assert!(refused(&format!("{NEW}) VALUES ({id}, 'max_x', 'Max', 1, 1, 1)")).await);
        assert!(refused(&format!("{NEW}) VALUES ({id}, 'max', '', 1, 1, 1)")).await);
        assert!(refused(&format!("{NEW}) VALUES (x'01', 'max', 'Max', 1, 1, 1)")).await);
        assert!(
            refused(&format!(
                "{NEW}, limit_week_pct) VALUES ({id}, 'max', 'Max', 1, 1, 1, 0)"
            ))
            .await
        );
        assert!(
            refused(&format!(
                "{NEW}, limit_five_hour_pct) VALUES ({id}, 'max', 'Max', 1, 1, 1, 101)"
            ))
            .await
        );
        assert!(
            refused("UPDATE claude_accounts SET config_dir = '/x' WHERE slug = 'default'").await
        );
        assert!(refused("UPDATE claude_accounts SET enabled = 2").await);
        assert!(!refused(&format!("{NEW}) VALUES ({id}, 'max-2', 'Max', 1, 1, 1)")).await);
    }
}
