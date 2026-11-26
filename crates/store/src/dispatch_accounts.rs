use std::collections::HashMap;

use async_trait::async_trait;
use nullradix_app::dispatch::accounts::{
    AccountPause, AccountPauseRepository, NewSample, PauseReason, UsageSampleRepository,
};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{Account, Id, Run, Timestamp, UsageSample, UsageSnapshot};
use uuid::Uuid;

use crate::error::database;
use crate::{Store, StoreError, change_feed};

/// Samples older than this are dropped whenever a new poll is kept.
const POLL_KEPT_MILLIS: i64 = 90 * 24 * 60 * 60 * 1000;

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid account {what}"))
}

fn pause_of(
    account: Uuid,
    until: i64,
    reason: &str,
    run: Option<Uuid>,
    created_at: i64,
) -> Result<AccountPause, AppError> {
    Ok(AccountPause {
        account: Id::from_uuid(account),
        until: Timestamp::from_millis(until),
        reason: reason
            .parse::<PauseReason>()
            .map_err(|_| corrupt("pause reason"))?,
        run: run.map(Id::from_uuid),
        created_at: Timestamp::from_millis(created_at),
    })
}

#[async_trait]
impl AccountPauseRepository for Store {
    async fn all(&self) -> Result<Vec<AccountPause>, AppError> {
        let rows = sqlx::query!(
            r#"SELECT account_id AS "account_id: Uuid", until, reason,
                      run_id AS "run_id?: Uuid", created_at
               FROM account_pauses ORDER BY until, account_id"#,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        rows.into_iter()
            .map(|row| {
                pause_of(
                    row.account_id,
                    row.until,
                    &row.reason,
                    row.run_id,
                    row.created_at,
                )
            })
            .collect()
    }

    async fn put(&self, pause: &AccountPause) -> Result<Change, AppError> {
        let account = pause.account.as_uuid();
        let run = pause.run.as_ref().map(Id::as_uuid);
        let until = pause.until.as_millis();
        let reason = pause.reason.as_str();
        let created_at = pause.created_at.as_millis();
        let mut tx = self.begin().await?;
        sqlx::query!(
            "INSERT INTO account_pauses (account_id, until, reason, run_id, created_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (account_id) DO UPDATE SET
               until = excluded.until, reason = excluded.reason, run_id = excluded.run_id,
               created_at = excluded.created_at",
            account,
            until,
            reason,
            run,
            created_at,
        )
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
        let removed = sqlx::query!("DELETE FROM account_pauses WHERE account_id = ?", uuid)
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
        let accounts = sqlx::query_scalar!(
            r#"DELETE FROM account_pauses RETURNING account_id AS "account_id: Uuid""#
        )
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

fn snapshot(
    five_hour: Option<i64>,
    seven_day: Option<i64>,
    five_hour_resets_at: Option<i64>,
    seven_day_resets_at: Option<i64>,
) -> UsageSnapshot {
    UsageSnapshot {
        five_hour,
        seven_day,
        five_hour_resets_at: five_hour_resets_at.map(Timestamp::from_millis),
        seven_day_resets_at: seven_day_resets_at.map(Timestamp::from_millis),
    }
}

async fn insert_sample(
    connection: &mut sqlx::SqliteConnection,
    sample: &NewSample,
) -> Result<u64, sqlx::Error> {
    let account = sample.account.as_uuid();
    let run = sample.run.as_ref().map(Id::as_uuid);
    let phase = sample.phase.as_str();
    let five_hour_resets_at = sample.usage.five_hour_resets_at.map(Timestamp::as_millis);
    let seven_day_resets_at = sample.usage.seven_day_resets_at.map(Timestamp::as_millis);
    let at = sample.at.as_millis();
    let done = sqlx::query!(
        "INSERT INTO account_usage_samples
           (id, account_id, run_id, phase, five_hour_pct, seven_day_pct, five_hour_resets_at,
            seven_day_resets_at, at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
        sample.id,
        account,
        run,
        phase,
        sample.usage.five_hour,
        sample.usage.seven_day,
        five_hour_resets_at,
        seven_day_resets_at,
        at,
    )
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
        let latest = sqlx::query!(
            "SELECT five_hour_pct, seven_day_pct, five_hour_resets_at, seven_day_resets_at
             FROM account_usage_samples WHERE account_id = ? ORDER BY at DESC, id DESC LIMIT 1",
            account,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(database)?;
        if let Some(row) = latest
            && snapshot(
                row.five_hour_pct,
                row.seven_day_pct,
                row.five_hour_resets_at,
                row.seven_day_resets_at,
            ) == sample.usage
        {
            return Ok(false);
        }
        insert_sample(&mut tx, sample).await.map_err(database)?;
        // start and end rows go with their run; older ones age out like polls
        let cutoff = sample.at.as_millis().saturating_sub(POLL_KEPT_MILLIS);
        sqlx::query!(
            "DELETE FROM account_usage_samples
             WHERE at < ? OR (phase <> 'poll' AND run_id IS NULL)",
            cutoff,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(true)
    }

    async fn history(&self, limit: u32) -> Result<Vec<UsageSample>, AppError> {
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            r#"SELECT r.profile_name AS "profile!", r.model AS model, r.effort AS effort,
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
               ORDER BY e.at DESC, e.id DESC LIMIT ?"#,
            limit,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        let mut samples: Vec<UsageSample> = rows
            .into_iter()
            .map(|row| UsageSample {
                profile: row.profile,
                model: row.model,
                effort: row.effort,
                started_at: Timestamp::from_millis(row.started_at),
                ended_at: Timestamp::from_millis(row.ended_at),
                before: snapshot(
                    row.b_five_hour_pct,
                    row.b_seven_day_pct,
                    row.b_five_hour_resets_at,
                    row.b_seven_day_resets_at,
                ),
                after: snapshot(
                    row.a_five_hour_pct,
                    row.a_seven_day_pct,
                    row.a_five_hour_resets_at,
                    row.a_seven_day_resets_at,
                ),
            })
            .collect();
        samples.reverse();
        Ok(samples)
    }

    async fn starts(&self, runs: &[Id<Run>]) -> Result<HashMap<Id<Run>, UsageSnapshot>, AppError> {
        let mut starts = HashMap::new();
        for run in runs {
            let uuid = run.as_uuid();
            let row = sqlx::query!(
                "SELECT five_hour_pct, seven_day_pct, five_hour_resets_at, seven_day_resets_at
                 FROM account_usage_samples WHERE run_id = ? AND phase = 'start'",
                uuid,
            )
            .fetch_optional(&self.reader)
            .await
            .map_err(database)?;
            if let Some(row) = row {
                starts.insert(
                    *run,
                    snapshot(
                        row.five_hour_pct,
                        row.seven_day_pct,
                        row.five_hour_resets_at,
                        row.seven_day_resets_at,
                    ),
                );
            }
        }
        Ok(starts)
    }
}
