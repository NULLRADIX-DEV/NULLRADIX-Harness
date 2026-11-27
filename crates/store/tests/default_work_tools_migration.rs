#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests fail loudly"
)]

use std::borrow::Cow;

use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

const MIGRATION: i64 = 20_261_127_030_000;

/// Frozen copies of the old and the new default; the live constant may change later.
const OLD_DEFAULT: &str = r#"["Read","Edit","Write","Glob","Grep","Bash(harness:*)","Bash(nullradix:*)","Bash(node:*)","Bash(npm test:*)","Bash(npm run:*)","Bash(git status:*)","Bash(git diff:*)","Bash(git log:*)","Bash(git branch:*)","Bash(git add:*)","Bash(git commit:*)"]"#;

const NEW_DEFAULT: &str = r#"["Read","Edit","Write","Glob","Grep","Bash(nullradix ticket show:*)","Bash(nullradix ticket list:*)","Bash(nullradix ticket comment:*)","Bash(nullradix ticket set:*)","Bash(nullradix ticket done:*)","Bash(nullradix step check:*)","Bash(nullradix step add:*)","Bash(nullradix step edit:*)","Bash(nullradix note add:*)","Bash(nullradix note list:*)","Bash(nullradix note seen:*)","Bash(nullradix note show:*)","Bash(nullradix question add:*)","Bash(nullradix question show:*)","Bash(nullradix question list:*)","Bash(nullradix question comment:*)","Bash(npm test:*)","Bash(npm run:*)","Bash(git status:*)","Bash(git diff:*)","Bash(git log:*)","Bash(git branch:*)","Bash(git add:*)","Bash(git commit:*)"]"#;

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

async fn tools(pool: &SqlitePool) -> String {
    sqlx::query("SELECT claude_allowed_tools FROM dispatcher_settings WHERE id = 1")
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn an_untouched_default_gets_the_explicit_nullradix_commands() {
    let pool = pool().await;
    before_migration().run(&pool).await.unwrap();
    assert_eq!(tools(&pool).await, OLD_DEFAULT);
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    assert_eq!(tools(&pool).await, NEW_DEFAULT);
}

#[tokio::test]
async fn a_list_somebody_changed_stays_as_it_is() {
    let pool = pool().await;
    before_migration().run(&pool).await.unwrap();
    let own = r#"["Read","Bash(nullradix:*)","Bash(make:*)"]"#;
    sqlx::query("UPDATE dispatcher_settings SET claude_allowed_tools = ? WHERE id = 1")
        .bind(own)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    assert_eq!(tools(&pool).await, own);
}
