use std::collections::HashMap;
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

/// Aborts and successes of the subjects in range, oldest first.
#[derive(Default)]
struct Earlier {
    aborts: HashMap<(String, RunKind), Vec<i64>>,
    successes: HashMap<String, Vec<i64>>,
}

impl Earlier {
    /// An abort of the same subject and kind lies after the last success before the run.
    fn retried(&self, subject: &str, kind: RunKind, at: i64) -> bool {
        let last_success = self.successes.get(subject).and_then(|times| {
            let before = times.partition_point(|time| *time < at);
            before
                .checked_sub(1)
                .and_then(|index| times.get(index))
                .copied()
        });
        let Some(aborts) = self.aborts.get(&(subject.to_owned(), kind)) else {
            return false;
        };
        let first = last_success.map_or(0, |success| {
            aborts.partition_point(|abort| *abort <= success)
        });
        aborts.get(first).is_some_and(|abort| *abort < at)
    }
}

impl Store {
    /// One read of the history of the subjects in range, by `ix_runs_subject`.
    async fn earlier_runs(&self, subjects: &[&str], until: i64) -> Result<Earlier, AppError> {
        let mut earlier = Earlier::default();
        if subjects.is_empty() {
            return Ok(earlier);
        }
        let subjects = serde_json::to_string(subjects).map_err(|_| corrupt("subject list"))?;
        let history = sqlx::query!(
            r#"SELECT subject_key AS "subject_key!", kind, outcome AS "outcome!", created_at
               FROM runs
               WHERE subject_key IN (SELECT value FROM json_each(?1))
                 AND outcome IN ('abort', 'abort-final', 'done', 'review', 'human',
                                 'merge-session', 'research')
                 AND created_at < ?2
               ORDER BY subject_key, created_at"#,
            subjects,
            until,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        for row in history {
            if matches!(row.outcome.as_str(), "abort" | "abort-final") {
                if let Some(kind) = row
                    .kind
                    .as_deref()
                    .and_then(|kind| RunKind::from_str(kind).ok())
                {
                    earlier
                        .aborts
                        .entry((row.subject_key, kind))
                        .or_default()
                        .push(row.created_at);
                }
            } else {
                earlier
                    .successes
                    .entry(row.subject_key)
                    .or_default()
                    .push(row.created_at);
            }
        }
        Ok(earlier)
    }
}

#[async_trait]
impl DispatchStatsRepository for Store {
    async fn runs(&self, query: &StatsQuery) -> Result<Vec<StatsRun>, AppError> {
        let (from, until) = (query.from.as_millis(), query.until.as_millis());
        let board = query.board.map(|board| board.as_uuid());
        let rows = sqlx::query!(
            r#"SELECT r.created_at, r.subject_key AS "subject_key!", r.kind AS "kind!",
                      r.profile_name, r.outcome, r.abort_category,
                      CASE WHEN r.started_at IS NOT NULL AND r.finished_at >= r.started_at
                           THEN r.finished_at - r.started_at END AS duration_ms,
                      r.cost_usd, r.input_tokens, r.output_tokens, r.cache_read_tokens,
                      r.cache_write_tokens
               FROM runs r
               WHERE r.kind IS NOT NULL AND r.kind <> 'plan' AND r.subject_key IS NOT NULL
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
        let mut subjects: Vec<&str> = rows.iter().map(|row| row.subject_key.as_str()).collect();
        subjects.sort_unstable();
        subjects.dedup();
        let earlier = self.earlier_runs(&subjects, until).await?;
        rows.into_iter()
            .map(|row| {
                let kind = RunKind::from_str(&row.kind).map_err(|_| corrupt("run kind"))?;
                let retried = earlier.retried(&row.subject_key, kind, row.created_at);
                Ok(StatsRun {
                    subject: row.subject_key,
                    kind,
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
                    retried,
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
