use std::collections::HashMap;

use async_trait::async_trait;
use nullradix_app::sessions::{
    SessionRepository, SessionScope, SessionTicket, SessionWrite, StoredSession, session_not_found,
};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    HookEvent, Id, ProjectKey, Repository, Runner, Session, SessionClient, SessionRecord,
    SessionRef, SessionState, Subagent, SubagentState, Ticket, TicketKey, TicketStatus, Timestamp,
    Version,
};
use sqlx::{Sqlite, SqliteConnection, Transaction};
use uuid::Uuid;

use crate::error::{database, missed_guard};
use crate::hooks::insert_event;
use crate::tickets::ticket_id;
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid session {what}"))
}

struct Row {
    id: String,
    repository_id: Option<Uuid>,
    runner_id: Option<Uuid>,
    number: Option<i64>,
    cwd: String,
    state: String,
    client: String,
    title: Option<String>,
    model: Option<String>,
    transcript_path: Option<String>,
    run_name: Option<String>,
    started_at: i64,
    last_event_at: i64,
    ended_at: Option<i64>,
    version: i64,
}

struct AgentRow {
    session_id: String,
    agent_id: String,
    agent_type: String,
    state: String,
    started_at: i64,
    ended_at: Option<i64>,
}

struct TicketRow {
    session_id: String,
    project_key: String,
    number: i64,
    title: String,
    status: String,
}

impl Row {
    fn into_session(self, subagents: Vec<Subagent>) -> Result<Session, StoreError> {
        let number = self
            .number
            .map(u32::try_from)
            .transpose()
            .map_err(|_| corrupt("number"))?;
        Ok(Session::restore(SessionRecord {
            id: SessionRef::parse(&self.id).map_err(|_| corrupt("id"))?,
            repository: self.repository_id.map(Id::from_uuid),
            runner: self.runner_id.map(Id::from_uuid),
            number,
            cwd: self.cwd,
            state: self
                .state
                .parse::<SessionState>()
                .map_err(|_| corrupt("state"))?,
            client: self
                .client
                .parse::<SessionClient>()
                .map_err(|_| corrupt("client"))?,
            title: self.title,
            model: self.model,
            transcript_path: self.transcript_path,
            run_name: self.run_name,
            started_at: Timestamp::from_millis(self.started_at),
            last_event_at: Timestamp::from_millis(self.last_event_at),
            ended_at: self.ended_at.map(Timestamp::from_millis),
            subagents,
            version: Version::from_stored(self.version),
        }))
    }
}

impl AgentRow {
    fn into_subagent(self) -> Result<(String, Subagent), StoreError> {
        let agent = Subagent {
            id: self.agent_id,
            kind: self.agent_type,
            state: self
                .state
                .parse::<SubagentState>()
                .map_err(|_| corrupt("subagent state"))?,
            started_at: Timestamp::from_millis(self.started_at),
            ended_at: self.ended_at.map(Timestamp::from_millis),
        };
        Ok((self.session_id, agent))
    }
}

impl TicketRow {
    fn into_ticket(self) -> Result<(String, SessionTicket), StoreError> {
        let project = ProjectKey::parse(&self.project_key).map_err(|_| corrupt("ticket"))?;
        let number = u32::try_from(self.number).map_err(|_| corrupt("ticket number"))?;
        let ticket = SessionTicket {
            key: TicketKey::new(project, number),
            title: self.title,
            status: self
                .status
                .parse::<TicketStatus>()
                .map_err(|_| corrupt("ticket status"))?,
        };
        Ok((self.session_id, ticket))
    }
}

/// Sessions in scope with subagents and tickets, latest activity first.
async fn load(
    connection: &mut SqliteConnection,
    scope: SessionScope,
    only: Option<&str>,
) -> Result<Vec<StoredSession>, StoreError> {
    let repository = scope.repository.map(|id| id.as_uuid());
    let ticket = scope.ticket.map(|id| id.as_uuid());
    let rows = sqlx::query_as!(
        Row,
        r#"SELECT s.id AS "id!", s.repository_id AS "repository_id?: Uuid",
                  s.runner_id AS "runner_id?: Uuid", s.number, s.cwd, s.state, s.client, s.title,
                  s.model, s.transcript_path, s.run_name, s.started_at, s.last_event_at,
                  s.ended_at, s.version
           FROM claude_sessions s
           WHERE (?1 IS NULL OR s.repository_id = ?1)
             AND (?2 IS NULL OR s.id IN (SELECT session_id FROM ticket_sessions
                                         WHERE ticket_id = ?2))
             AND (?3 IS NULL OR s.id = ?3)
           ORDER BY s.last_event_at DESC, s.touched DESC"#,
        repository,
        ticket,
        only,
    )
    .fetch_all(&mut *connection)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let agents = sqlx::query_as!(
        AgentRow,
        r#"SELECT a.session_id, a.agent_id, a.agent_type, a.state, a.started_at, a.ended_at
           FROM session_subagents a JOIN claude_sessions s ON s.id = a.session_id
           WHERE (?1 IS NULL OR s.repository_id = ?1)
             AND (?2 IS NULL OR s.id IN (SELECT session_id FROM ticket_sessions
                                         WHERE ticket_id = ?2))
             AND (?3 IS NULL OR s.id = ?3)
           ORDER BY a.session_id, a.position"#,
        repository,
        ticket,
        only,
    )
    .fetch_all(&mut *connection)
    .await?;
    let links = sqlx::query_as!(
        TicketRow,
        r#"SELECT ts.session_id, p.key AS project_key, t.number, t.title, t.status
           FROM ticket_sessions ts
           JOIN claude_sessions s ON s.id = ts.session_id
           JOIN tickets t ON t.id = ts.ticket_id
           JOIN projects p ON p.id = t.project_id
           WHERE (?1 IS NULL OR s.repository_id = ?1)
             AND (?2 IS NULL OR s.id IN (SELECT session_id FROM ticket_sessions
                                         WHERE ticket_id = ?2))
             AND (?3 IS NULL OR s.id = ?3)
           ORDER BY p.key, t.number"#,
        repository,
        ticket,
        only,
    )
    .fetch_all(&mut *connection)
    .await?;
    let mut subagents: HashMap<String, Vec<Subagent>> = HashMap::new();
    for row in agents {
        let (session, agent) = row.into_subagent()?;
        subagents.entry(session).or_default().push(agent);
    }
    let mut tickets: HashMap<String, Vec<SessionTicket>> = HashMap::new();
    for row in links {
        let (session, ticket) = row.into_ticket()?;
        tickets.entry(session).or_default().push(ticket);
    }
    rows.into_iter()
        .map(|row| {
            let agents = subagents.remove(&row.id).unwrap_or_default();
            let linked = tickets.remove(&row.id).unwrap_or_default();
            Ok(StoredSession {
                session: row.into_session(agents)?,
                tickets: linked,
            })
        })
        .collect()
}

pub(crate) async fn one(
    connection: &mut SqliteConnection,
    id: &SessionRef,
) -> Result<Option<StoredSession>, StoreError> {
    Ok(load(connection, SessionScope::default(), Some(id.as_str()))
        .await?
        .pop())
}

async fn save_subagents(
    connection: &mut SqliteConnection,
    session: &Session,
) -> Result<(), StoreError> {
    let id = session.id().as_str();
    sqlx::query!("DELETE FROM session_subagents WHERE session_id = ?", id)
        .execute(&mut *connection)
        .await?;
    for (position, agent) in session.record().subagents.iter().enumerate() {
        let position = i64::try_from(position).map_err(|_| corrupt("subagent position"))?;
        let state = agent.state.as_str();
        let started_at = agent.started_at.as_millis();
        let ended_at = agent.ended_at.map(Timestamp::as_millis);
        sqlx::query!(
            "INSERT INTO session_subagents (session_id, agent_id, position, agent_type, state,
                                            started_at, ended_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            id,
            agent.id,
            position,
            agent.kind,
            state,
            started_at,
            ended_at,
        )
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

struct Columns<'a> {
    id: &'a str,
    repository: Option<Uuid>,
    runner: Option<Uuid>,
    number: Option<i64>,
    state: &'static str,
    client: &'static str,
    started_at: i64,
    last_event_at: i64,
    ended_at: Option<i64>,
    version: i64,
}

impl<'a> Columns<'a> {
    fn of(session: &'a Session) -> Self {
        let record = session.record();
        Self {
            id: record.id.as_str(),
            repository: record.repository.map(|id| id.as_uuid()),
            runner: record.runner.map(|id| id.as_uuid()),
            number: record.number.map(i64::from),
            state: record.state.as_str(),
            client: record.client.as_str(),
            started_at: record.started_at.as_millis(),
            last_event_at: record.last_event_at.as_millis(),
            ended_at: record.ended_at.map(Timestamp::as_millis),
            version: record.version.value(),
        }
    }
}

pub(crate) struct Write(Transaction<'static, Sqlite>);

#[async_trait]
impl SessionWrite for Write {
    async fn record_hook(
        &mut self,
        event: &HookEvent,
        received_at: Timestamp,
    ) -> Result<bool, AppError> {
        Ok(insert_event(&mut self.0, event, received_at).await?)
    }

    async fn session(&mut self, id: &SessionRef) -> Result<Option<Session>, AppError> {
        Ok(one(&mut self.0, id).await?.map(|stored| stored.session))
    }

    async fn insert(&mut self, session: &Session, feed_id: Uuid) -> Result<Change, AppError> {
        let record = session.record();
        let columns = Columns::of(session);
        sqlx::query!(
            "INSERT INTO claude_sessions (id, feed_id, repository_id, runner_id, number, cwd, state,
                                   client, title, model, transcript_path, run_name, started_at,
                                   last_event_at, ended_at, touched, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                     (SELECT coalesce(max(touched), 0) + 1 FROM claude_sessions), ?)",
            columns.id,
            feed_id,
            columns.repository,
            columns.runner,
            columns.number,
            record.cwd,
            columns.state,
            columns.client,
            record.title,
            record.model,
            record.transcript_path,
            record.run_name,
            columns.started_at,
            columns.last_event_at,
            columns.ended_at,
            columns.version,
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        save_subagents(&mut self.0, session).await?;
        Ok(change_feed::record(
            &mut self.0,
            Entity::Session,
            feed_id,
            ChangeOp::Insert,
            record.last_event_at,
        )
        .await?)
    }

    async fn update(&mut self, session: &Session, previous: Version) -> Result<Change, AppError> {
        let record = session.record();
        let columns = Columns::of(session);
        let previous = previous.value();
        let updated = sqlx::query_scalar!(
            r#"UPDATE claude_sessions
               SET repository_id = ?, runner_id = ?, number = ?, cwd = ?, state = ?, client = ?,
                   title = ?, model = ?, transcript_path = ?, run_name = ?, started_at = ?,
                   last_event_at = ?, ended_at = ?,
                   touched = (SELECT coalesce(max(touched), 0) + 1 FROM claude_sessions), version = ?
               WHERE id = ? AND version = ?
               RETURNING feed_id AS "feed_id: Uuid""#,
            columns.repository,
            columns.runner,
            columns.number,
            record.cwd,
            columns.state,
            columns.client,
            record.title,
            record.model,
            record.transcript_path,
            record.run_name,
            columns.started_at,
            columns.last_event_at,
            columns.ended_at,
            columns.version,
            columns.id,
            previous,
        )
        .fetch_optional(&mut *self.0)
        .await
        .map_err(database)?;
        let Some(feed_id) = updated else {
            let exists = sqlx::query_scalar!(
                "SELECT version FROM claude_sessions WHERE id = ?",
                columns.id
            )
            .fetch_optional(&mut *self.0)
            .await;
            return Err(missed_guard(
                exists,
                AppError::version_conflict("session", columns.id),
                session_not_found(columns.id),
            ));
        };
        save_subagents(&mut self.0, session).await?;
        Ok(change_feed::record(
            &mut self.0,
            Entity::Session,
            feed_id,
            ChangeOp::Update,
            record.last_event_at,
        )
        .await?)
    }

    async fn touch(&mut self, id: &SessionRef, at: Timestamp) -> Result<Change, AppError> {
        let id = id.as_str();
        let feed_id = sqlx::query_scalar!(
            r#"SELECT feed_id AS "feed_id: Uuid" FROM claude_sessions WHERE id = ?"#,
            id
        )
        .fetch_one(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(
            change_feed::record(&mut self.0, Entity::Session, feed_id, ChangeOp::Update, at)
                .await?,
        )
    }

    async fn next_number(&mut self, repository: Id<Repository>) -> Result<Option<u32>, AppError> {
        let repository = repository.as_uuid();
        let number = sqlx::query_scalar!(
            r#"UPDATE repositories SET next_session_number = next_session_number + 1
               WHERE id = ? RETURNING next_session_number - 1 AS "number!: i64""#,
            repository,
        )
        .fetch_optional(&mut *self.0)
        .await
        .map_err(database)?;
        number
            .map(u32::try_from)
            .transpose()
            .map_err(|_| corrupt("next number").into())
    }

    async fn ticket(&mut self, key: &TicketKey) -> Result<Option<Id<Ticket>>, AppError> {
        Ok(ticket_id(&mut self.0, key).await?.map(Id::from_uuid))
    }

    async fn link(
        &mut self,
        ticket: Id<Ticket>,
        session: &SessionRef,
        at: Timestamp,
    ) -> Result<bool, AppError> {
        let ticket = ticket.as_uuid();
        let session = session.as_str();
        let at = at.as_millis();
        let inserted = sqlx::query!(
            "INSERT INTO ticket_sessions (ticket_id, session_id, added_at) VALUES (?, ?, ?)
             ON CONFLICT DO NOTHING",
            ticket,
            session,
            at,
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(inserted.rows_affected() == 1)
    }

    async fn linked(&mut self, session: &SessionRef) -> Result<bool, AppError> {
        let session = session.as_str();
        let linked = sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM ticket_sessions WHERE session_id = ?) AS "linked!: bool""#,
            session,
        )
        .fetch_one(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(linked)
    }

    async fn worktree_tickets(
        &mut self,
        runner: Id<Runner>,
    ) -> Result<Vec<(String, Id<Ticket>)>, AppError> {
        let runner = runner.as_uuid();
        let rows = sqlx::query!(
            r#"SELECT w.path, wt.ticket_id AS "ticket_id: Uuid"
               FROM worktrees w
               JOIN worktree_tickets wt ON wt.worktree_id = w.id
               JOIN tickets t ON t.id = wt.ticket_id
               WHERE w.runner_id = ? AND w.desired_state = 'present'
                 AND wt.released_at IS NULL AND t.status = 'in-progress'
               ORDER BY w.created_at, wt.position"#,
            runner,
        )
        .fetch_all(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(rows
            .into_iter()
            .map(|row| (row.path, Id::from_uuid(row.ticket_id)))
            .collect())
    }

    async fn unlocated(&mut self, cwd: &str) -> Result<Vec<Session>, AppError> {
        let ids = sqlx::query_scalar!(
            r#"SELECT id AS "id!" FROM claude_sessions WHERE cwd = ? AND repository_id IS NULL
               ORDER BY started_at, touched"#,
            cwd,
        )
        .fetch_all(&mut *self.0)
        .await
        .map_err(database)?;
        let mut sessions = Vec::new();
        for id in ids {
            let id = SessionRef::parse(&id).map_err(|_| corrupt("id"))?;
            if let Some(stored) = one(&mut self.0, &id).await? {
                sessions.push(stored.session);
            }
        }
        Ok(sessions)
    }

    async fn commit(self: Box<Self>) -> Result<(), AppError> {
        self.0.commit().await.map_err(database)
    }
}

#[async_trait]
impl SessionRepository for Store {
    async fn begin(&self) -> Result<Box<dyn SessionWrite>, AppError> {
        Ok(Box::new(Write(self.begin().await?)))
    }

    async fn ticket_id(&self, key: &TicketKey) -> Result<Option<Id<Ticket>>, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        Ok(ticket_id(&mut read, key).await?.map(Id::from_uuid))
    }

    async fn list(&self, scope: SessionScope) -> Result<Vec<StoredSession>, AppError> {
        // one read transaction keeps sessions, subagents and links on the same snapshot
        let mut read = self.reader.begin().await.map_err(database)?;
        let sessions = load(&mut read, scope, None).await?;
        read.commit().await.map_err(database)?;
        Ok(sessions)
    }

    async fn get(&self, id: &SessionRef) -> Result<Option<StoredSession>, AppError> {
        let mut read = self.reader.begin().await.map_err(database)?;
        let session = one(&mut read, id).await?;
        read.commit().await.map_err(database)?;
        Ok(session)
    }

    async fn prune_before(
        &self,
        before: Timestamp,
        now: Timestamp,
    ) -> Result<(u64, Option<Change>), AppError> {
        let millis = before.as_millis();
        let mut write = self.begin().await?;
        let gone = sqlx::query_scalar!(
            r#"DELETE FROM claude_sessions WHERE coalesce(ended_at, last_event_at) < ?
               RETURNING feed_id AS "feed_id: Uuid""#,
            millis,
        )
        .fetch_all(&mut *write)
        .await
        .map_err(database)?;
        let mut last = None;
        for feed_id in &gone {
            last = Some(
                change_feed::record(&mut write, Entity::Session, *feed_id, ChangeOp::Delete, now)
                    .await?,
            );
        }
        write.commit().await.map_err(database)?;
        Ok((u64::try_from(gone.len()).unwrap_or(u64::MAX), last))
    }
}
