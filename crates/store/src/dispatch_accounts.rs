use std::collections::HashMap;

use async_trait::async_trait;
use nullradix_app::dispatch::accounts::{
    AccountPause, AccountPauseRepository, NewSample, PauseReason, UsageSampleRepository,
};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{Account, Id, Run, Timestamp, UsageSample, UsageSnapshot};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use uuid::Uuid;

use crate::error::database;
use crate::{Store, StoreError, change_feed};

/// Samples older than this are dropped whenever a new poll is kept.
const POLL_KEPT_MILLIS: i64 = 90 * 24 * 60 * 60 * 1000;

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid account {what}"))
}

fn pause_of(row: &SqliteRow) -> Result<AccountPause, AppError> {
    let reason: String = row.try_get("reason").map_err(database)?;
    let account: Uuid = row.try_get("account_id").map_err(database)?;
    let run: Option<Uuid> = row.try_get("run_id").map_err(database)?;
    Ok(AccountPause {
        account: Id::from_uuid(account),
        until: Timestamp::from_millis(row.try_get("until").map_err(database)?),
        reason: reason
            .parse::<PauseReason>()
            .map_err(|_| corrupt("pause reason"))?,
        run: run.map(Id::from_uuid),
        created_at: Timestamp::from_millis(row.try_get("created_at").map_err(database)?),
    })
}

#[async_trait]
impl AccountPauseRepository for Store {
    async fn all(&self) -> Result<Vec<AccountPause>, AppError> {
        let rows = sqlx::query(
            "SELECT account_id, until, reason, run_id, created_at FROM account_pauses
             ORDER BY until, account_id",
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        rows.iter().map(pause_of).collect()
    }

    async fn put(&self, pause: &AccountPause) -> Result<Change, AppError> {
        let account = pause.account.as_uuid();
        let run = pause.run.as_ref().map(Id::as_uuid);
        let mut tx = self.begin().await?;
        sqlx::query(
            "INSERT INTO account_pauses (account_id, until, reason, run_id, created_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (account_id) DO UPDATE SET
               until = excluded.until, reason = excluded.reason, run_id = excluded.run_id,
               created_at = excluded.created_at",
        )
        .bind(account)
        .bind(pause.until.as_millis())
        .bind(pause.reason.as_str())
        .bind(run)
        .bind(pause.created_at.as_millis())
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        let change = change_feed::record(
            &mut tx,
            Entity::Account,
            account,
            ChangeOp::Update,
            pause.created_at,
        )
        .await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn remove(
        &self,
        account: Id<Account>,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let uuid = account.as_uuid();
        let mut tx = self.begin().await?;
        let removed = sqlx::query("DELETE FROM account_pauses WHERE account_id = ?")
            .bind(uuid)
            .execute(&mut *tx)
            .await
            .map_err(database)?;
        if removed.rows_affected() == 0 {
            return Ok(None);
        }
        let change =
            change_feed::record(&mut tx, Entity::Account, uuid, ChangeOp::Update, at).await?;
        tx.commit().await.map_err(database)?;
        Ok(Some(change))
    }

    async fn clear(&self, at: Timestamp) -> Result<Vec<Change>, AppError> {
        let mut tx = self.begin().await?;
        let accounts: Vec<Uuid> =
            sqlx::query_scalar("DELETE FROM account_pauses RETURNING account_id")
                .fetch_all(&mut *tx)
                .await
                .map_err(database)?;
        let mut changes = Vec::with_capacity(accounts.len());
        for account in accounts {
            changes.push(
                change_feed::record(&mut tx, Entity::Account, account, ChangeOp::Update, at)
                    .await?,
            );
        }
        tx.commit().await.map_err(database)?;
        Ok(changes)
    }
}

fn snapshot_of(row: &SqliteRow, prefix: &str) -> Result<UsageSnapshot, AppError> {
    let column = |name: &str| format!("{prefix}{name}");
    let at = |name: &str| -> Result<Option<Timestamp>, AppError> {
        let millis: Option<i64> = row.try_get(column(name).as_str()).map_err(database)?;
        Ok(millis.map(Timestamp::from_millis))
    };
    Ok(UsageSnapshot {
        five_hour: row
            .try_get(column("five_hour_pct").as_str())
            .map_err(database)?,
        seven_day: row
            .try_get(column("seven_day_pct").as_str())
            .map_err(database)?,
        five_hour_resets_at: at("five_hour_resets_at")?,
        seven_day_resets_at: at("seven_day_resets_at")?,
    })
}

async fn insert_sample(
    connection: &mut sqlx::SqliteConnection,
    sample: &NewSample,
) -> Result<u64, sqlx::Error> {
    let run = sample.run.as_ref().map(Id::as_uuid);
    let done = sqlx::query(
        "INSERT INTO account_usage_samples
           (id, account_id, run_id, phase, five_hour_pct, seven_day_pct, five_hour_resets_at,
            seven_day_resets_at, at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
    )
    .bind(sample.id)
    .bind(sample.account.as_uuid())
    .bind(run)
    .bind(sample.phase.as_str())
    .bind(sample.usage.five_hour)
    .bind(sample.usage.seven_day)
    .bind(sample.usage.five_hour_resets_at.map(Timestamp::as_millis))
    .bind(sample.usage.seven_day_resets_at.map(Timestamp::as_millis))
    .bind(sample.at.as_millis())
    .execute(connection)
    .await?;
    Ok(done.rows_affected())
}

#[async_trait]
impl UsageSampleRepository for Store {
    async fn record(&self, sample: &NewSample) -> Result<(), AppError> {
        let mut tx = self.begin().await?;
        insert_sample(&mut tx, sample).await.map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(())
    }

    async fn record_poll(&self, sample: &NewSample) -> Result<bool, AppError> {
        let account = sample.account.as_uuid();
        let mut tx = self.begin().await?;
        let latest = sqlx::query(
            "SELECT five_hour_pct, seven_day_pct, five_hour_resets_at, seven_day_resets_at
             FROM account_usage_samples WHERE account_id = ? ORDER BY at DESC, id DESC LIMIT 1",
        )
        .bind(account)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database)?;
        if let Some(row) = latest
            && snapshot_of(&row, "")? == sample.usage
        {
            return Ok(false);
        }
        insert_sample(&mut tx, sample).await.map_err(database)?;
        // start and end rows go with their run; older ones age out like polls
        sqlx::query(
            "DELETE FROM account_usage_samples
             WHERE at < ? OR (phase <> 'poll' AND run_id IS NULL)",
        )
        .bind(sample.at.as_millis().saturating_sub(POLL_KEPT_MILLIS))
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(true)
    }

    async fn history(&self, limit: u32) -> Result<Vec<UsageSample>, AppError> {
        let rows = sqlx::query(
            "SELECT r.profile_name AS profile, r.model AS model, r.effort AS effort,
                    s.at AS started_at, e.at AS ended_at,
                    s.five_hour_pct AS b_five_hour_pct, s.seven_day_pct AS b_seven_day_pct,
                    s.five_hour_resets_at AS b_five_hour_resets_at,
                    s.seven_day_resets_at AS b_seven_day_resets_at,
                    e.five_hour_pct AS a_five_hour_pct, e.seven_day_pct AS a_seven_day_pct,
                    e.five_hour_resets_at AS a_five_hour_resets_at,
                    e.seven_day_resets_at AS a_seven_day_resets_at
             FROM account_usage_samples e
             JOIN account_usage_samples s ON s.run_id = e.run_id AND s.phase = 'start'
             JOIN runs r ON r.id = e.run_id
             WHERE e.phase = 'end' AND r.profile_name IS NOT NULL
             ORDER BY e.at DESC, e.id DESC LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        let mut samples = rows
            .iter()
            .map(|row| {
                Ok(UsageSample {
                    profile: row.try_get("profile").map_err(database)?,
                    model: row.try_get("model").map_err(database)?,
                    effort: row.try_get("effort").map_err(database)?,
                    started_at: Timestamp::from_millis(
                        row.try_get("started_at").map_err(database)?,
                    ),
                    ended_at: Timestamp::from_millis(row.try_get("ended_at").map_err(database)?),
                    before: snapshot_of(row, "b_")?,
                    after: snapshot_of(row, "a_")?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        samples.reverse();
        Ok(samples)
    }

    async fn starts(&self, runs: &[Id<Run>]) -> Result<HashMap<Id<Run>, UsageSnapshot>, AppError> {
        let mut starts = HashMap::new();
        for run in runs {
            let row = sqlx::query(
                "SELECT five_hour_pct, seven_day_pct, five_hour_resets_at, seven_day_resets_at
                 FROM account_usage_samples WHERE run_id = ? AND phase = 'start'",
            )
            .bind(run.as_uuid())
            .fetch_optional(&self.reader)
            .await
            .map_err(database)?;
            if let Some(row) = row {
                starts.insert(*run, snapshot_of(&row, "")?);
            }
        }
        Ok(starts)
    }
}
