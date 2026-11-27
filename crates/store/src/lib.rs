//! SQLite persistence of the daemon.

mod accounts;
mod auth;
mod auto_runs;
mod boards;
mod change_feed;
mod claude_theme;
mod comments;
mod customizations;
mod dispatch_accounts;
mod dispatch_stats;
mod dispatcher_config;
mod dispatcher_state;
mod error;
mod favorites;
mod hooks;
mod inbox;
mod merge_jobs;
mod notes;
mod projects;
mod questions;
mod repositories;
mod reviews;
mod runs;
mod sessions;
mod snippets;
mod theme_catalog;
mod themes;
mod tickets;
mod worktrees;

use std::path::Path;
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::{Sqlite, Transaction};

pub use error::StoreError;

static MIGRATOR: Migrator = sqlx::migrate!();

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const READERS: u32 = 8;

/// Handle to the database; cheap to clone.
#[derive(Debug, Clone)]
pub struct Store {
    writer: SqlitePool,
    reader: SqlitePool,
}

impl Store {
    /// Opens or creates the database file and applies pending migrations.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let base = SqliteConnectOptions::new()
            .filename(path)
            .foreign_keys(true)
            .busy_timeout(BUSY_TIMEOUT);
        let writer_options = base
            .clone()
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);
        // one writer connection queues writes in order instead of racing on the busy timeout
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(writer_options)
            .await?;
        MIGRATOR.run(&writer).await?;
        let reader = SqlitePoolOptions::new()
            .max_connections(READERS)
            .connect_with(base.read_only(true))
            .await?;
        Ok(Self { writer, reader })
    }

    /// Private in-memory database for tests.
    pub async fn in_memory() -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true)
            .busy_timeout(BUSY_TIMEOUT);
        // the one connection is shared, so a stalled machine must not turn waiting into failing
        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .acquire_timeout(Duration::from_mins(2))
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self {
            writer: pool.clone(),
            reader: pool,
        })
    }

    pub async fn close(&self) {
        self.reader.close().await;
        self.writer.close().await;
    }

    /// Write transaction that takes the lock up front instead of failing on upgrade.
    pub(crate) async fn begin(&self) -> Result<Transaction<'static, Sqlite>, StoreError> {
        Ok(self.writer.begin_with("BEGIN IMMEDIATE").await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn opens_a_file_database_in_wal_mode_with_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("nullradix.db")).await.unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&store.reader)
            .await
            .unwrap();
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&store.reader)
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        assert_eq!(foreign_keys, 1);
        store.close().await;
    }

    #[tokio::test]
    async fn reopening_keeps_applied_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nullradix.db");
        Store::open(&path).await.unwrap().close().await;
        let store = Store::open(&path).await.unwrap();
        let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(&store.reader)
            .await
            .unwrap();
        assert_eq!(applied, i64::try_from(MIGRATOR.iter().count()).unwrap());
        store.close().await;
    }

    #[tokio::test]
    async fn the_macos_migration_keeps_every_row_that_points_at_a_runner() {
        const BEFORE: i64 = 20_261_125_050_000;
        let dir = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(dir.path().join("nullradix.db"))
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        MIGRATOR.run_to(BEFORE, &pool).await.unwrap();
        for sql in [
            "INSERT INTO runners (id, name, os, created_at, updated_at) VALUES (x'01010101010101010101010101010101', 'local', 'linux', 1, 1)",
            "INSERT INTO repositories (id, name, created_at, updated_at) VALUES (x'02020202020202020202020202020202', 'arches', 1, 1)",
            "INSERT INTO checkouts (id, repository_id, runner_id, path, path_key, created_at, updated_at) VALUES (x'03030303030303030303030303030303', x'02020202020202020202020202020202', x'01010101010101010101010101010101', '/work/arches', '/work/arches', 1, 1)",
            "INSERT INTO worktrees (id, repository_id, runner_id, branch, path, desired_state, actual_state, created_at, updated_at) VALUES (x'04040404040404040404040404040404', x'02020202020202020202020202020202', x'01010101010101010101010101010101', 'ARC-1', '/work/arches-wt', 'present', 'present', 1, 1)",
            "INSERT INTO claude_sessions (id, feed_id, repository_id, runner_id, cwd, state, started_at, last_event_at, touched) VALUES ('s-1', x'05050505050505050505050505050505', x'02020202020202020202020202020202', x'01010101010101010101010101010101', '/work/arches', 'idle', 1, 1, 1)",
        ] {
            sqlx::query(sql).execute(&pool).await.unwrap();
        }
        MIGRATOR.run(&pool).await.unwrap();

        let kept_rows: (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM checkouts WHERE runner_id = x'01010101010101010101010101010101'),
                    (SELECT count(*) FROM worktrees WHERE runner_id = x'01010101010101010101010101010101'),
                    (SELECT count(*) FROM claude_sessions WHERE runner_id = x'01010101010101010101010101010101')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(kept_rows, (1, 1, 1));
        let kept: (String, String, i64) = sqlx::query_as("SELECT name, os, version FROM runners")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(kept, ("local".to_owned(), "linux".to_owned(), 1));
        let dangling = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(dangling.is_empty());
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(foreign_keys, 1);

        let add = |id: &str, os: &str| {
            sqlx::query(
                "INSERT INTO runners (id, name, os, created_at, updated_at) VALUES (?1, ?2, ?2, 1, 1)",
            )
            .bind(id.as_bytes().to_vec())
            .bind(os.to_owned())
            .execute(&pool)
        };
        assert!(add("0123456789abcdef", "macos").await.is_ok());
        assert!(add("fedcba9876543210", "plan9").await.is_err());
        let referenced = sqlx::query("DELETE FROM runners WHERE name = 'local'")
            .execute(&pool)
            .await;
        assert!(referenced.is_err());
        pool.close().await;
    }

    #[tokio::test]
    async fn concurrent_writers_commit_one_after_another() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("nullradix.db")).await.unwrap();
        let write = |store: Store, id: u128| async move {
            let mut tx = store.begin().await.unwrap();
            let at = nullradix_domain::Timestamp::from_millis(1);
            let change = change_feed::record(
                &mut tx,
                nullradix_app::Entity::Project,
                uuid::Uuid::from_u128(id),
                nullradix_app::ChangeOp::Insert,
                at,
            )
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.commit().await.unwrap();
            change.seq
        };
        let first = tokio::spawn(write(store.clone(), 1));
        let second = tokio::spawn(write(store.clone(), 2));
        let mut seqs = vec![first.await.unwrap(), second.await.unwrap()];
        seqs.sort_unstable();
        assert_eq!(seqs, vec![1, 2]);
        store.close().await;
    }
}
