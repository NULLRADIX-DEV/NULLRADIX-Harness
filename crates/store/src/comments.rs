use async_trait::async_trait;
use nullradix_app::comments::{CommentLine, CommentRepository, CommentWrite};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    ActorKind, Comment, CommentBody, CommentRecord, Id, ProjectKey, SystemMarker, Ticket,
    TicketKey, Timestamp, Version,
};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::error::database;
use crate::tickets::{Write, ticket_id};
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid {what}"))
}

struct Row {
    id: Uuid,
    ticket_id: Uuid,
    project_key: String,
    number: i64,
    author_id: Uuid,
    author_name: String,
    author_kind: String,
    body: String,
    system_marker: Option<String>,
    run_id: Option<Uuid>,
    delivered_to_agent_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

impl Row {
    fn into_line(self) -> Result<CommentLine, StoreError> {
        let project = ProjectKey::parse(&self.project_key).map_err(|_| corrupt("project key"))?;
        let number = u32::try_from(self.number).map_err(|_| corrupt("ticket number"))?;
        let system_marker = self
            .system_marker
            .map(|marker| marker.parse::<SystemMarker>())
            .transpose()
            .map_err(|_| corrupt("system marker"))?;
        let record = CommentRecord {
            id: Id::from_uuid(self.id),
            ticket: Id::from_uuid(self.ticket_id),
            author: Id::from_uuid(self.author_id),
            body: CommentBody::parse(&self.body).map_err(|_| corrupt("comment body"))?,
            system_marker,
            run: self.run_id.map(Id::from_uuid),
            delivered_at: self.delivered_to_agent_at.map(Timestamp::from_millis),
            created_at: Timestamp::from_millis(self.created_at),
            updated_at: Timestamp::from_millis(self.updated_at),
            version: Version::from_stored(self.version),
        };
        Ok(CommentLine {
            comment: Comment::restore(record),
            ticket_key: TicketKey::new(project, number),
            author_name: self.author_name,
            author_kind: self
                .author_kind
                .parse::<ActorKind>()
                .map_err(|_| corrupt("actor kind"))?,
        })
    }
}

/// Comments of the ticket oldest first; `undelivered` keeps those no agent got yet.
pub(crate) async fn ticket_comments(
    connection: &mut SqliteConnection,
    ticket: Uuid,
    undelivered: bool,
) -> Result<Vec<CommentLine>, StoreError> {
    let rows = sqlx::query_as!(
        Row,
        r#"SELECT c.id AS "id: Uuid", c.ticket_id AS "ticket_id: Uuid", p.key AS project_key,
                  t.number, c.author_id AS "author_id: Uuid", a.name AS author_name,
                  a.kind AS author_kind, c.body, c.system_marker, c.run_id AS "run_id: Uuid",
                  c.delivered_to_agent_at, c.created_at, c.updated_at, c.version
           FROM comments c
           JOIN tickets t ON t.id = c.ticket_id
           JOIN projects p ON p.id = t.project_id
           JOIN actors a ON a.id = c.author_id
           WHERE c.ticket_id = ?1 AND (?2 = 0 OR c.delivered_to_agent_at IS NULL)
           ORDER BY c.created_at, c.rid"#,
        ticket,
        undelivered,
    )
    .fetch_all(connection)
    .await?;
    rows.into_iter().map(Row::into_line).collect()
}

#[async_trait]
impl CommentWrite for Write {
    async fn insert_comment(&mut self, comment: &Comment) -> Result<Change, AppError> {
        let record = comment.record();
        let id = record.id.as_uuid();
        let ticket = record.ticket.as_uuid();
        let author = record.author.as_uuid();
        let marker = record.system_marker.map(SystemMarker::as_str);
        let run = record.run.map(|run| run.as_uuid());
        let delivered = record.delivered_at.map(Timestamp::as_millis);
        let created_at = record.created_at.as_millis();
        let updated_at = record.updated_at.as_millis();
        let version = record.version.value();
        sqlx::query!(
            "INSERT INTO comments (id, ticket_id, author_id, body, system_marker, run_id,
                                   delivered_to_agent_at, created_at, updated_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            ticket,
            author,
            record.body.as_str(),
            marker,
            run,
            delivered,
            created_at,
            updated_at,
            version,
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(change_feed::record(
            &mut self.0,
            Entity::Comment,
            id,
            ChangeOp::Insert,
            record.created_at,
        )
        .await?)
    }
}

#[async_trait]
impl CommentRepository for Store {
    async fn ticket_exists(&self, ticket: &TicketKey) -> Result<bool, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        Ok(ticket_id(&mut read, ticket).await?.is_some())
    }

    async fn list(&self, ticket: &TicketKey) -> Result<Option<Vec<CommentLine>>, AppError> {
        let mut read = self.reader.begin().await.map_err(database)?;
        let Some(id) = ticket_id(&mut read, ticket).await? else {
            return Ok(None);
        };
        let comments = ticket_comments(&mut read, id, false).await?;
        read.commit().await.map_err(database)?;
        Ok(Some(comments))
    }

    async fn undelivered(&self, tickets: &[Id<Ticket>]) -> Result<Vec<CommentLine>, AppError> {
        let mut read = self.reader.begin().await.map_err(database)?;
        let mut found = Vec::new();
        for ticket in tickets {
            found.extend(ticket_comments(&mut read, ticket.as_uuid(), true).await?);
        }
        read.commit().await.map_err(database)?;
        found.sort_by_key(|line| line.comment.record().created_at);
        Ok(found)
    }

    async fn mark_delivered(
        &self,
        ids: &[Id<Comment>],
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let millis = at.as_millis();
        let mut write = self.begin().await?;
        let mut last = None;
        for id in ids {
            let uuid = id.as_uuid();
            let marked = sqlx::query!(
                "UPDATE comments SET delivered_to_agent_at = ?
                 WHERE id = ? AND delivered_to_agent_at IS NULL",
                millis,
                uuid,
            )
            .execute(&mut *write)
            .await
            .map_err(database)?;
            if marked.rows_affected() > 0 {
                let change =
                    change_feed::record(&mut write, Entity::Comment, uuid, ChangeOp::Update, at)
                        .await?;
                last = Some(change);
            }
        }
        write.commit().await.map_err(database)?;
        Ok(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn comment_rows(store: &Store) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM comments")
            .fetch_one(&store.reader)
            .await
            .unwrap()
    }

    async fn run(store: &Store, sql: &'static str) {
        let id = |n: u128| Uuid::from_u128(n);
        sqlx::query(sql)
            .bind(id(1))
            .bind(id(2))
            .bind(id(3))
            .bind(id(4))
            .execute(&store.writer)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn deleting_a_ticket_deletes_its_comments() {
        let store = Store::in_memory().await.unwrap();
        // ?1 actor, ?2 project, ?3 board, ?4 ticket
        run(
            &store,
            "INSERT INTO actors (id, kind, name, created_at) VALUES (?1, 'human', 'anna', 0)",
        )
        .await;
        run(&store, "INSERT INTO projects (id, key, name, created_at, updated_at) VALUES (?2, 'ARC', 'Arches', 0, 0)").await;
        run(&store, "INSERT INTO boards (id, project_id, name, owner_id, created_at, updated_at) VALUES (?3, ?2, 'Main', ?1, 0, 0)").await;
        run(&store, "INSERT INTO tickets (id, project_id, board_id, number, title, description, acceptance, plan, handbook, diagram, status, priority, sort_order, needs_planning, created_by, created_at, updated_at) VALUES (?4, ?2, ?3, 1, 'Gone', '', '', '', '', '', 'backlog', 'normal', 0, 0, ?1, 0, 0)").await;
        for (n, body) in [(5_u128, "soon gone"), (6, "me too")] {
            sqlx::query("INSERT INTO comments (id, ticket_id, author_id, body, created_at, updated_at) VALUES (?, ?, ?, ?, 0, 0)")
                .bind(Uuid::from_u128(n))
                .bind(Uuid::from_u128(4))
                .bind(Uuid::from_u128(1))
                .bind(body)
                .execute(&store.writer)
                .await
                .unwrap();
        }
        assert_eq!(comment_rows(&store).await, 2);

        run(&store, "DELETE FROM tickets WHERE id = ?4").await;

        assert_eq!(comment_rows(&store).await, 0);
    }
}
