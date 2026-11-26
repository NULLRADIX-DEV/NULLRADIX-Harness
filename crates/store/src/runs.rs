use async_trait::async_trait;
use nullradix_app::runs::{self, RunArtifact, RunDetails, RunFilter, RunRepository};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    Abort, AbortCategory, DeniedTool, DispatchOutcome, DispatchRun, Id, ModelUsage, Run, RunKind,
    RunLabel, RunProfile, RunRecord, RunResult, RunStatus, RunTrigger, RunUsage, Screenshot,
    Timestamp, Version,
};
use serde_json::{Map, Value, json};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::error::{database, missed_guard};
use crate::{Store, StoreError, change_feed};

struct Row {
    id: Uuid,
    label: String,
    status: String,
    pid: Option<i64>,
    exit_code: Option<i64>,
    error: Option<String>,
    created_at: i64,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    version: i64,
    kind: Option<String>,
    triggered_by: Option<String>,
    subject_key: Option<String>,
    profile_id: Option<Uuid>,
    profile_name: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    max_turns: Option<i64>,
    timeout_minutes: Option<i64>,
    account_id: Option<Uuid>,
    config_hash: Option<String>,
    agent: Option<String>,
    cwd: Option<String>,
    command_args: Option<String>,
    prompt: Option<String>,
    claude_session_id: Option<String>,
    abort_category: Option<String>,
    abort_reason: Option<String>,
    num_turns: Option<i64>,
    cost_usd: Option<f64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    model_usage: Option<String>,
    denied_count: Option<i64>,
    outcome: Option<String>,
}

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid run {what}"))
}

fn small(value: Option<i64>, what: &str) -> Result<Option<u32>, StoreError> {
    value
        .map(u32::try_from)
        .transpose()
        .map_err(|_| corrupt(what))
}

fn models_json(models: &[ModelUsage]) -> Option<String> {
    if models.is_empty() {
        return None;
    }
    let map: Map<String, Value> = models
        .iter()
        .map(|model| {
            (
                model.model.clone(),
                json!({
                    "inputTokens": model.input_tokens,
                    "outputTokens": model.output_tokens,
                    "cacheReadTokens": model.cache_read_tokens,
                    "cacheWriteTokens": model.cache_write_tokens,
                    "costUsd": model.cost_usd,
                }),
            )
        })
        .collect();
    Some(Value::Object(map).to_string())
}

fn models_from(text: Option<&str>) -> Result<Vec<ModelUsage>, StoreError> {
    let Some(text) = text else {
        return Ok(Vec::new());
    };
    let Value::Object(map) = serde_json::from_str(text).map_err(|_| corrupt("model usage"))? else {
        return Err(corrupt("model usage"));
    };
    Ok(map
        .iter()
        .map(|(model, value)| ModelUsage {
            model: model.clone(),
            input_tokens: value.get("inputTokens").and_then(Value::as_i64),
            output_tokens: value.get("outputTokens").and_then(Value::as_i64),
            cache_read_tokens: value.get("cacheReadTokens").and_then(Value::as_i64),
            cache_write_tokens: value.get("cacheWriteTokens").and_then(Value::as_i64),
            cost_usd: value.get("costUsd").and_then(Value::as_f64),
        })
        .collect())
}

impl Row {
    fn dispatch(&self) -> Result<Option<DispatchRun>, StoreError> {
        let Some(kind) = &self.kind else {
            return Ok(None);
        };
        let args = match &self.command_args {
            Some(text) => serde_json::from_str(text).map_err(|_| corrupt("arguments"))?,
            None => Vec::new(),
        };
        Ok(Some(DispatchRun {
            kind: kind.parse::<RunKind>().map_err(|_| corrupt("kind"))?,
            trigger: self
                .triggered_by
                .as_deref()
                .unwrap_or("manual")
                .parse::<RunTrigger>()
                .map_err(|_| corrupt("trigger"))?,
            subject_key: self.subject_key.clone(),
            profile: RunProfile {
                id: self.profile_id,
                name: self.profile_name.clone().unwrap_or_default(),
                model: self.model.clone(),
                effort: self.effort.clone(),
                max_turns: small(self.max_turns, "max turns")?.unwrap_or(0),
                timeout_minutes: small(self.timeout_minutes, "timeout")?.unwrap_or(0),
            },
            account: self.account_id,
            config_hash: self.config_hash.clone().unwrap_or_default(),
            agent: self.agent.clone(),
            cwd: self.cwd.clone().unwrap_or_default(),
            args,
            prompt: self.prompt.clone().unwrap_or_default(),
        }))
    }

    fn usage(&self) -> Result<Option<RunUsage>, StoreError> {
        let any = [
            self.num_turns,
            self.input_tokens,
            self.output_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
        ]
        .iter()
        .any(Option::is_some)
            || self.cost_usd.is_some()
            || self.model_usage.is_some();
        if !any {
            return Ok(None);
        }
        let models = models_from(self.model_usage.as_deref())?;
        // the main model is the one that cost the most, first on a tie
        let mut main: Option<&ModelUsage> = None;
        for model in &models {
            if main
                .is_none_or(|best| model.cost_usd.unwrap_or(-1.0) > best.cost_usd.unwrap_or(-1.0))
            {
                main = Some(model);
            }
        }
        Ok(Some(RunUsage {
            cost_usd: self.cost_usd,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
            num_turns: self.num_turns,
            model: main.map(|model| model.model.clone()),
            models,
        }))
    }

    fn result(&self) -> Result<Option<RunResult>, StoreError> {
        let abort = match &self.abort_category {
            Some(category) => Some(Abort {
                category: category
                    .parse::<AbortCategory>()
                    .map_err(|_| corrupt("abort category"))?,
                reason: self.abort_reason.clone().unwrap_or_default(),
            }),
            None => None,
        };
        let outcome = self
            .outcome
            .as_deref()
            .map(|outcome| {
                outcome
                    .parse::<DispatchOutcome>()
                    .map_err(|_| corrupt("outcome"))
            })
            .transpose()?;
        let result = RunResult {
            session_id: self.claude_session_id.clone(),
            usage: self.usage()?,
            denied_count: small(self.denied_count, "denied count")?,
            abort,
            outcome,
        };
        let empty = result.session_id.is_none()
            && result.usage.is_none()
            && result.denied_count.is_none()
            && result.abort.is_none()
            && result.outcome.is_none();
        Ok((!empty).then_some(result))
    }

    fn into_run(self) -> Result<Run, StoreError> {
        let label = RunLabel::parse(&self.label).map_err(|_| corrupt("label"))?;
        let status = self
            .status
            .parse::<RunStatus>()
            .map_err(|_| corrupt("status"))?;
        let pid = self
            .pid
            .map(u32::try_from)
            .transpose()
            .map_err(|_| corrupt("pid"))?;
        let exit_code = self
            .exit_code
            .map(i32::try_from)
            .transpose()
            .map_err(|_| corrupt("exit code"))?;
        let dispatch = self.dispatch()?;
        let result = self.result()?;
        Ok(Run::restore(RunRecord {
            id: Id::from_uuid(self.id),
            label,
            status,
            pid,
            exit_code,
            error: self.error,
            created_at: Timestamp::from_millis(self.created_at),
            started_at: self.started_at.map(Timestamp::from_millis),
            finished_at: self.finished_at.map(Timestamp::from_millis),
            dispatch,
            result,
            version: Version::from_stored(self.version),
        }))
    }
}

async fn missed(connection: &mut SqliteConnection, id: Id<Run>) -> AppError {
    let uuid = id.as_uuid();
    let exists = sqlx::query_scalar!("SELECT 1 FROM runs WHERE id = ?", uuid)
        .fetch_optional(connection)
        .await;
    missed_guard(
        exists,
        AppError::version_conflict("run", id),
        runs::not_found(id),
    )
}

fn changed_at(run: &Run) -> Timestamp {
    run.finished_at()
        .or(run.started_at())
        .unwrap_or_else(|| run.created_at())
}

fn runs_of(rows: Vec<Row>) -> Result<Vec<Run>, AppError> {
    Ok(rows
        .into_iter()
        .map(Row::into_run)
        .collect::<Result<_, _>>()?)
}

/// Denied tools and artifacts of a finished run; writing them twice changes nothing.
async fn write_details(
    connection: &mut SqliteConnection,
    id: Uuid,
    details: &RunDetails,
) -> Result<(), AppError> {
    for tool in &details.denied {
        let name = tool.tool.as_str();
        let count = i64::from(tool.count);
        sqlx::query!(
            "INSERT OR IGNORE INTO run_denied_tools (run_id, tool, count) VALUES (?, ?, ?)",
            id,
            name,
            count,
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    }
    for artifact in &details.artifacts {
        let artifact_id = artifact.id;
        let file = artifact.screenshot.file.as_str();
        let media_type = artifact.screenshot.media_type.as_str();
        let bytes = i64::try_from(artifact.screenshot.bytes)
            .map_err(|_| StoreError::Corrupt(String::from("artifact too large")))?;
        let tool_use_id = artifact.screenshot.tool_use_id.as_deref();
        let created_at = artifact.created_at.as_millis();
        sqlx::query!(
            "INSERT OR IGNORE INTO run_artifacts
                (id, run_id, kind, file, media_type, bytes, tool_use_id, created_at)
             VALUES (?, ?, 'screenshot', ?, ?, ?, ?, ?)",
            artifact_id,
            id,
            file,
            media_type,
            bytes,
            tool_use_id,
            created_at,
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    }
    Ok(())
}

#[async_trait]
impl RunRepository for Store {
    async fn insert(&self, run: &Run) -> Result<Change, AppError> {
        let id = run.id().as_uuid();
        let label = run.label().as_str();
        let status = run.status().as_str();
        let pid = run.pid().map(i64::from);
        let exit_code = run.exit_code().map(i64::from);
        let error = run.error();
        let created_at = run.created_at().as_millis();
        let started_at = run.started_at().map(Timestamp::as_millis);
        let finished_at = run.finished_at().map(Timestamp::as_millis);
        let version = run.version().value();
        let dispatch = run.dispatch();
        let kind = dispatch.map(|dispatch| dispatch.kind.as_str());
        let triggered_by = dispatch.map(|dispatch| dispatch.trigger.as_str());
        let subject_key = dispatch.and_then(|dispatch| dispatch.subject_key.as_deref());
        let profile_id = dispatch.and_then(|dispatch| dispatch.profile.id);
        let profile_name = dispatch.map(|dispatch| dispatch.profile.name.as_str());
        let model = dispatch.and_then(|dispatch| dispatch.profile.model.as_deref());
        let effort = dispatch.and_then(|dispatch| dispatch.profile.effort.as_deref());
        let max_turns = dispatch.map(|dispatch| i64::from(dispatch.profile.max_turns));
        let timeout_minutes = dispatch.map(|dispatch| i64::from(dispatch.profile.timeout_minutes));
        let account_id = dispatch.and_then(|dispatch| dispatch.account);
        let config_hash = dispatch.map(|dispatch| dispatch.config_hash.as_str());
        let agent = dispatch.and_then(|dispatch| dispatch.agent.as_deref());
        let cwd = dispatch.map(|dispatch| dispatch.cwd.as_str());
        let command_args = dispatch
            .map(|dispatch| serde_json::to_string(&dispatch.args))
            .transpose()
            .map_err(|error| StoreError::Corrupt(error.to_string()))?;
        let prompt = dispatch.map(|dispatch| dispatch.prompt.as_str());
        let mut tx = self.begin().await?;
        sqlx::query!(
            "INSERT INTO runs (id, label, status, pid, exit_code, error, created_at, started_at,
                               finished_at, version, kind, triggered_by, subject_key, profile_id,
                               profile_name, model, effort, max_turns, timeout_minutes, account_id,
                               config_hash, cwd, command_args, prompt, agent)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            label,
            status,
            pid,
            exit_code,
            error,
            created_at,
            started_at,
            finished_at,
            version,
            kind,
            triggered_by,
            subject_key,
            profile_id,
            profile_name,
            model,
            effort,
            max_turns,
            timeout_minutes,
            account_id,
            config_hash,
            cwd,
            command_args,
            prompt,
            agent,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        let change =
            change_feed::record(&mut tx, Entity::Run, id, ChangeOp::Insert, run.created_at())
                .await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn update_with(
        &self,
        run: &Run,
        expected: Version,
        details: &RunDetails,
    ) -> Result<Change, AppError> {
        let id = run.id().as_uuid();
        let status = run.status().as_str();
        let pid = run.pid().map(i64::from);
        let exit_code = run.exit_code().map(i64::from);
        let error = run.error();
        let started_at = run.started_at().map(Timestamp::as_millis);
        let finished_at = run.finished_at().map(Timestamp::as_millis);
        let version = run.version().value();
        let expected = expected.value();
        let result = run.result();
        let claude_session_id = result.and_then(|result| result.session_id.as_deref());
        let denied_count = result.and_then(|result| result.denied_count).map(i64::from);
        let abort_category = result
            .and_then(|result| result.abort.as_ref())
            .map(|abort| abort.category.as_str());
        let outcome = result
            .and_then(|result| result.outcome)
            .map(DispatchOutcome::as_str);
        let abort_reason = result
            .and_then(|result| result.abort.as_ref())
            .map(|abort| abort.reason.as_str());
        let usage = result.and_then(|result| result.usage.as_ref());
        let num_turns = usage.and_then(|usage| usage.num_turns);
        let cost_usd = usage.and_then(|usage| usage.cost_usd);
        let input_tokens = usage.and_then(|usage| usage.input_tokens);
        let output_tokens = usage.and_then(|usage| usage.output_tokens);
        let cache_read_tokens = usage.and_then(|usage| usage.cache_read_tokens);
        let cache_write_tokens = usage.and_then(|usage| usage.cache_write_tokens);
        let model_usage = usage.and_then(|usage| models_json(&usage.models));
        let mut tx = self.begin().await?;
        let updated = sqlx::query!(
            "UPDATE runs SET status = ?, pid = ?, exit_code = ?, error = ?, started_at = ?,
                             finished_at = ?, version = ?, claude_session_id = ?,
                             denied_count = ?, abort_category = ?, abort_reason = ?,
                             num_turns = ?, cost_usd = ?, input_tokens = ?, output_tokens = ?,
                             cache_read_tokens = ?, cache_write_tokens = ?, model_usage = ?, outcome = ?
             WHERE id = ? AND version = ?",
            status,
            pid,
            exit_code,
            error,
            started_at,
            finished_at,
            version,
            claude_session_id,
            denied_count,
            abort_category,
            abort_reason,
            num_turns,
            cost_usd,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            model_usage,
            outcome,
            id,
            expected,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if updated.rows_affected() == 0 {
            return Err(missed(&mut tx, run.id()).await);
        }
        write_details(&mut tx, id, details).await?;
        let change =
            change_feed::record(&mut tx, Entity::Run, id, ChangeOp::Update, changed_at(run))
                .await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn get(&self, id: Id<Run>) -> Result<Option<Run>, AppError> {
        let uuid = id.as_uuid();
        let row = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", label, status, pid, exit_code, error, created_at,
                      started_at, finished_at, version, kind, triggered_by, subject_key,
                      profile_id AS "profile_id: Uuid", profile_name, model, effort, max_turns,
                      timeout_minutes, account_id AS "account_id: Uuid", config_hash, cwd,
                      command_args, prompt, claude_session_id, abort_category, abort_reason,
                      num_turns, cost_usd, input_tokens, output_tokens, cache_read_tokens,
                      cache_write_tokens, model_usage, denied_count, outcome, agent
               FROM runs WHERE id = ?"#,
            uuid
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(row.map(Row::into_run).transpose()?)
    }

    async fn list(&self, limit: u32) -> Result<Vec<Run>, AppError> {
        self.find(&RunFilter::default(), limit).await
    }

    async fn find(&self, filter: &RunFilter, limit: u32) -> Result<Vec<Run>, AppError> {
        let subject = filter.subject_key.as_deref();
        let kind = filter.kind.map(RunKind::as_str);
        let session = filter
            .link
            .as_ref()
            .map(|link| link.claude_session.as_str());
        let run = filter
            .link
            .as_ref()
            .and_then(|link| link.run)
            .map(|run| run.as_uuid());
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", label, status, pid, exit_code, error, created_at,
                      started_at, finished_at, version, kind, triggered_by, subject_key,
                      profile_id AS "profile_id: Uuid", profile_name, model, effort, max_turns,
                      timeout_minutes, account_id AS "account_id: Uuid", config_hash, cwd,
                      command_args, prompt, claude_session_id, abort_category, abort_reason,
                      num_turns, cost_usd, input_tokens, output_tokens, cache_read_tokens,
                      cache_write_tokens, model_usage, denied_count, outcome, agent
               FROM runs WHERE (?1 IS NULL OR subject_key = ?1)
                  AND (?2 IS NULL OR kind = ?2)
                  AND (?3 IS NULL OR claude_session_id = ?3 OR id = ?4)
                ORDER BY created_at DESC, id DESC LIMIT ?5"#,
            subject,
            kind,
            session,
            run,
            limit,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        runs_of(rows)
    }

    async fn active(&self) -> Result<Vec<Run>, AppError> {
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", label, status, pid, exit_code, error, created_at,
                      started_at, finished_at, version, kind, triggered_by, subject_key,
                      profile_id AS "profile_id: Uuid", profile_name, model, effort, max_turns,
                      timeout_minutes, account_id AS "account_id: Uuid", config_hash, cwd,
                      command_args, prompt, claude_session_id, abort_category, abort_reason,
                      num_turns, cost_usd, input_tokens, output_tokens, cache_read_tokens,
                      cache_write_tokens, model_usage, denied_count, outcome, agent
               FROM runs WHERE status IN ('starting', 'running') ORDER BY created_at"#
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        runs_of(rows)
    }

    async fn unevaluated(&self, limit: u32) -> Result<Vec<Run>, AppError> {
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", label, status, pid, exit_code, error, created_at,
                      started_at, finished_at, version, kind, triggered_by, subject_key,
                      profile_id AS "profile_id: Uuid", profile_name, model, effort, max_turns,
                      timeout_minutes, account_id AS "account_id: Uuid", config_hash, cwd,
                      command_args, prompt, claude_session_id, abort_category, abort_reason,
                      num_turns, cost_usd, input_tokens, output_tokens, cache_read_tokens,
                      cache_write_tokens, model_usage, denied_count, outcome, agent
               FROM runs
               WHERE kind = 'work' AND subject_key IS NOT NULL AND outcome IS NULL
                 AND status NOT IN ('starting', 'running')
                 AND (abort_category IS NULL OR abort_category <> 'start-failed')
               ORDER BY finished_at, id LIMIT ?"#,
            limit,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        runs_of(rows)
    }

    async fn denied_tools(&self, id: Id<Run>) -> Result<Vec<DeniedTool>, AppError> {
        let uuid = id.as_uuid();
        let rows = sqlx::query!(
            "SELECT tool, count FROM run_denied_tools WHERE run_id = ? ORDER BY count DESC, tool",
            uuid,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        rows.into_iter()
            .map(|row| {
                Ok(DeniedTool {
                    tool: row.tool,
                    count: u32::try_from(row.count).map_err(|_| corrupt("denied count"))?,
                })
            })
            .collect::<Result<_, StoreError>>()
            .map_err(AppError::from)
    }

    async fn artifacts(&self, id: Id<Run>) -> Result<Vec<RunArtifact>, AppError> {
        let uuid = id.as_uuid();
        let rows = sqlx::query!(
            r#"SELECT id AS "id: Uuid", file, media_type, bytes, tool_use_id, created_at
               FROM run_artifacts WHERE run_id = ? ORDER BY created_at, id"#,
            uuid,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        rows.into_iter()
            .map(|row| {
                Ok(RunArtifact {
                    id: row.id,
                    screenshot: Screenshot {
                        tool_use_id: row.tool_use_id,
                        media_type: row.media_type,
                        file: row.file,
                        bytes: u64::try_from(row.bytes).map_err(|_| corrupt("artifact size"))?,
                    },
                    created_at: Timestamp::from_millis(row.created_at),
                })
            })
            .collect::<Result<_, StoreError>>()
            .map_err(AppError::from)
    }
}
