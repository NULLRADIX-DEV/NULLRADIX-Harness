#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests fail loudly"
)]

use std::borrow::Cow;

use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

const MIGRATION: i64 = 20_261_127_010_000;

async fn pool() -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .in_memory(true)
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options)
        .await
        .unwrap()
}

fn before_migration() -> Migrator {
    let all = sqlx::migrate!("./migrations");
    let kept = all
        .migrations
        .iter()
        .filter(|migration| migration.version < MIGRATION)
        .cloned()
        .collect::<Vec<_>>();
    Migrator {
        migrations: Cow::Owned(kept),
        ..Migrator::DEFAULT
    }
}

async fn columns(pool: &SqlitePool) -> Vec<String> {
    sqlx::query("SELECT name || ':' || type || ':' || \"notnull\" || ':' || pk FROM pragma_table_info('runs')")
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect()
}

async fn insert(pool: &SqlitePool, id: u8, outcome: Option<&str>) {
    sqlx::query(
        "INSERT INTO runs (id, label, status, created_at, kind, subject_key, outcome, abort_category, \
         abort_reason, agent, cost_usd) VALUES (?, 'work', 'failed', ?, 'work', 'ARC-1', ?, \
         'start-failed', 'no', 'a', 1.5)",
    )
    .bind(vec![id; 16])
    .bind(i64::from(id))
    .bind(outcome)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn the_rebuild_keeps_runs_and_what_refers_to_them_and_accepts_the_new_outcome() {
    let pool = pool().await;
    before_migration().run(&pool).await.unwrap();
    insert(&pool, 1, Some("pause-claude-missing")).await;
    insert(&pool, 2, Some("abort")).await;
    insert(&pool, 3, None).await;
    sqlx::query("INSERT INTO run_denied_tools (run_id, tool, count) VALUES (?, 'Bash', 3)")
        .bind(vec![1_u8; 16])
        .execute(&pool)
        .await
        .unwrap();
    let before = columns(&pool).await;
    let rejected = sqlx::query("UPDATE runs SET outcome = 'start-failed' WHERE created_at = 3")
        .execute(&pool)
        .await;
    assert!(rejected.is_err(), "the old list does not know the value");

    sqlx::migrate!("./migrations").run(&pool).await.unwrap();

    assert_eq!(columns(&pool).await, before);
    let rows: Vec<(i64, Option<String>, String, f64)> = sqlx::query_as(
        "SELECT created_at, outcome, abort_reason, cost_usd FROM runs ORDER BY created_at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            (1, Some("pause-claude-missing".into()), "no".into(), 1.5),
            (2, Some("abort".into()), "no".into(), 1.5),
            (3, None, "no".into(), 1.5),
        ]
    );
    let denied: i64 = sqlx::query_scalar("SELECT count FROM run_denied_tools WHERE tool = 'Bash'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(denied, 3, "rows that refer to a run survive the rebuild");
    let broken: i64 = sqlx::query_scalar("SELECT count(*) FROM pragma_foreign_key_check")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(broken, 0);
    let keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(keys, 1);
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'runs' \
         AND name LIKE 'ix_runs_%' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        indexes,
        [
            "ix_runs_active",
            "ix_runs_claude_session",
            "ix_runs_config",
            "ix_runs_created_at",
            "ix_runs_subject"
        ]
    );
    insert(&pool, 4, Some("start-failed")).await;
    let bogus = sqlx::query("UPDATE runs SET outcome = 'nonsense' WHERE created_at = 4")
        .execute(&pool)
        .await;
    assert!(bogus.is_err(), "the list still guards the column");
    // deleting a run still cascades to what it owns
    sqlx::query("DELETE FROM runs WHERE created_at = 1")
        .execute(&pool)
        .await
        .unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM run_denied_tools")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn the_rebuild_stops_on_a_dangling_reference() {
    let pool = pool().await;
    before_migration().run(&pool).await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO run_denied_tools (run_id, tool, count) VALUES (?, 'Bash', 1)")
        .bind(vec![9_u8; 16])
        .execute(&pool)
        .await
        .unwrap();
    let applied = sqlx::migrate!("./migrations").run(&pool).await;
    assert!(applied.is_err(), "a dangling reference aborts the rebuild");
}
