//! SQLite persistence of the daemon.

mod auth;
mod boards;
mod change_feed;
mod error;
mod hooks;
mod projects;
mod runs;

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
        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
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
    pub(crate) async fn begin(&self) -> Result<Transaction<'_, Sqlite>, StoreError> {
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
