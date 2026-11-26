use std::str::FromStr;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::dispatch::stats::{DispatchStatsRepository, StatsQuery};
use nullradix_domain::{AbortCategory, DispatchOutcome, RunKind, StatsRun, Timestamp};
use uuid::Uuid;

use crate::error::database;
use crate::{Store, StoreError};

fn corrupt(what: &str) -> AppError {
    AppError::from(StoreError::Corrupt(format!("invalid {what}")))
}

#[async_trait]
impl DispatchStatsRepository for Store {
    async fn runs(&self, query: &StatsQuery) -> Result<Vec<StatsRun>, AppError> {
        let (from, until) = (query.from.as_millis(), query.until.as_millis());
        let board = query.board.map(|board| board.as_uuid());
        let rows = sqlx::query!(
            r#"SELECT r.created_at, r.subject_key AS "subject_key!", r.kind AS "kind!",
                      r.profile_name, r.outcome, r.abort_category,
                      EXISTS (SELECT 1 FROM runs e
                              WHERE e.subject_key = r.subject_key AND e.kind = r.kind
                                AND e.outcome IN ('abort', 'abort-final')
                                AND e.created_at < r.created_at) AS "retried!: bool",
                      CASE WHEN r.started_at IS NOT NULL AND r.finished_at >= r.started_at
                           THEN r.finished_at - r.started_at END AS duration_ms,
                      r.cost_usd, r.input_tokens, r.output_tokens, r.cache_read_tokens,
                      r.cache_write_tokens
               FROM runs r
               WHERE r.kind IS NOT NULL AND r.subject_key IS NOT NULL
                 AND r.created_at >= ?1 AND r.created_at < ?2
                 AND (?3 IS NULL OR r.kind = 'research'
                      OR r.subject_key IN (SELECT p.key || '-' || printf('%04d', t.number)
                                           FROM tickets t JOIN projects p ON p.id = t.project_id
                                           WHERE t.board_id = ?3))
               ORDER BY r.created_at, r.id
               LIMIT ?4"#,
            from,
            until,
            board as Option<Uuid>,
            query.limit,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        rows.into_iter()
            .map(|row| {
                Ok(StatsRun {
                    subject: row.subject_key,
                    kind: RunKind::from_str(&row.kind).map_err(|_| corrupt("run kind"))?,
                    profile: row.profile_name.unwrap_or_default(),
                    outcome: row
                        .outcome
                        .as_deref()
                        .map(DispatchOutcome::from_str)
                        .transpose()
                        .map_err(|_| corrupt("run outcome"))?,
                    abort_category: row
                        .abort_category
                        .as_deref()
                        .map(AbortCategory::from_str)
                        .transpose()
                        .map_err(|_| corrupt("abort category"))?,
                    retried: row.retried,
                    started_at: Timestamp::from_millis(row.created_at),
                    duration_ms: row.duration_ms,
                    cost_usd: row.cost_usd,
                    input_tokens: row.input_tokens,
                    output_tokens: row.output_tokens,
                    cache_read_tokens: row.cache_read_tokens,
                    cache_write_tokens: row.cache_write_tokens,
                })
            })
            .collect()
    }
}
