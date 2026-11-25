use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::hooks::HookRepository;
use nullradix_domain::{HookDetails, HookEvent, HookKind, Id, SessionRef, Timestamp};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::error::database;
use crate::{Store, StoreError};

struct Row {
    id: Uuid,
    kind: String,
    session_id: String,
    cwd: String,
    occurred_at: i64,
    transcript_path: Option<String>,
    prompt: Option<String>,
    source: Option<String>,
    model: Option<String>,
    reason: Option<String>,
    message: Option<String>,
    notification_type: Option<String>,
    agent_id: Option<String>,
    agent_type: Option<String>,
    ticket: Option<String>,
    run: Option<String>,
    entrypoint: Option<String>,
}

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid hook event {what}"))
}

impl Row {
    fn into_event(self) -> Result<HookEvent, StoreError> {
        let kind = self.kind.parse::<HookKind>().map_err(|_| corrupt("kind"))?;
        let session = SessionRef::parse(&self.session_id).map_err(|_| corrupt("session"))?;
        let details = HookDetails {
            transcript_path: self.transcript_path,
            prompt: self.prompt,
            source: self.source,
            model: self.model,
            reason: self.reason,
            message: self.message,
            notification_type: self.notification_type,
            agent_id: self.agent_id,
            agent_type: self.agent_type,
            ticket: self.ticket,
            run: self.run,
            entrypoint: self.entrypoint,
        };
        HookEvent::new(
            Id::from_uuid(self.id),
            kind,
            session,
            &self.cwd,
            Timestamp::from_millis(self.occurred_at),
            details,
        )
        .map_err(|_| corrupt("cwd"))
    }
}

/// Keeps the event unless one with its id exists; true when it was new.
pub(crate) async fn insert_event(
    connection: &mut SqliteConnection,
    event: &HookEvent,
    received_at: Timestamp,
) -> Result<bool, StoreError> {
    let id = event.id().as_uuid();
    let kind = event.kind().as_str();
    let session = event.session().as_str();
    let cwd = event.cwd();
    let occurred_at = event.occurred_at().as_millis();
    let received_at = received_at.as_millis();
    let details = event.details();
    let inserted = sqlx::query!(
        "INSERT INTO hook_events (id, kind, session_id, cwd, occurred_at, received_at,
                                      transcript_path, prompt, source, model, reason, message,
                                      notification_type, agent_id, agent_type, ticket, run,
                                      entrypoint)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO NOTHING",
        id,
        kind,
        session,
        cwd,
        occurred_at,
        received_at,
        details.transcript_path,
        details.prompt,
        details.source,
        details.model,
        details.reason,
        details.message,
        details.notification_type,
        details.agent_id,
        details.agent_type,
        details.ticket,
        details.run,
        details.entrypoint,
    )
    .execute(connection)
    .await?;
    Ok(inserted.rows_affected() == 1)
}

#[async_trait]
impl HookRepository for Store {
    async fn record(&self, event: &HookEvent, received_at: Timestamp) -> Result<bool, AppError> {
        let mut connection = self.writer.acquire().await.map_err(database)?;
        Ok(insert_event(&mut connection, event, received_at).await?)
    }

    async fn recent(&self, limit: u32) -> Result<Vec<HookEvent>, AppError> {
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id: Uuid", kind, session_id, cwd, occurred_at, transcript_path,
                      prompt, source, model, reason, message, notification_type, agent_id,
                      agent_type, ticket, run, entrypoint
               FROM hook_events ORDER BY occurred_at DESC, id DESC LIMIT ?"#,
            limit,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows
            .into_iter()
            .map(Row::into_event)
            .collect::<Result<_, _>>()?)
    }

    async fn prune_before(&self, before: Timestamp) -> Result<u64, AppError> {
        let before = before.as_millis();
        let deleted = sqlx::query!("DELETE FROM hook_events WHERE received_at < ?", before)
            .execute(&self.writer)
            .await
            .map_err(database)?;
        Ok(deleted.rows_affected())
    }
}
