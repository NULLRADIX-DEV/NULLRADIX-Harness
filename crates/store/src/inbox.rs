use std::collections::HashMap;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::inbox::{
    InboxBoard, InboxNote, InboxQuestion, InboxRepository, InboxSnapshot, InboxTicket,
};
use nullradix_domain::{
    Id, InboxRef, Priority, ProjectKey, QuestionKey, TicketKey, TicketStatus, Timestamp,
};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::error::database;
use crate::{Store, StoreError};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid {what}"))
}

fn project_key(text: &str) -> Result<ProjectKey, StoreError> {
    ProjectKey::parse(text).map_err(|_| corrupt("project key"))
}

fn number(value: i64) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| corrupt("number"))
}

async fn boards(connection: &mut SqliteConnection) -> Result<Vec<InboxBoard>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT b.id AS "id: Uuid", b.name, p.key AS project_key
           FROM boards b
           JOIN projects p ON p.id = b.project_id
           ORDER BY b.created_at, b.id"#
    )
    .fetch_all(connection)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(InboxBoard {
                id: Id::from_uuid(row.id),
                name: row.name,
                project_key: project_key(&row.project_key)?,
            })
        })
        .collect()
}

/// Tickets that wait themselves or that an open question is about.
async fn tickets(connection: &mut SqliteConnection) -> Result<Vec<InboxTicket>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT t.id AS "id: Uuid", p.key AS project_key, t.number, t.board_id AS "board_id: Uuid",
                  t.title, t.status, t.priority, t.blocked_reason, t.updated_at
           FROM tickets t
           JOIN projects p ON p.id = t.project_id
           WHERE t.status IN ('blocked', 'in-planning')
              OR t.id IN (SELECT qt.ticket_id
                          FROM question_tickets qt
                          JOIN questions q ON q.id = qt.question_id
                          WHERE q.state = 'open')
           ORDER BY p.key, t.number"#
    )
    .fetch_all(connection)
    .await?;
    rows.into_iter()
        .map(|row| {
            let key = TicketKey::new(project_key(&row.project_key)?, number(row.number)?);
            Ok(InboxTicket {
                id: Id::from_uuid(row.id),
                key,
                board: Id::from_uuid(row.board_id),
                title: row.title,
                status: TicketStatus::parse(&row.status).map_err(|_| corrupt("ticket status"))?,
                priority: Priority::parse(&row.priority).map_err(|_| corrupt("priority"))?,
                blocked_reason: row.blocked_reason,
                updated_at: Timestamp::from_millis(row.updated_at),
            })
        })
        .collect()
}

async fn questions(connection: &mut SqliteConnection) -> Result<Vec<InboxQuestion>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT q.id AS "id: Uuid", p.key AS project_key, q.number, q.title, q.occasion,
                  q.created_at
           FROM questions q
           JOIN projects p ON p.id = q.project_id
           WHERE q.state = 'open'
           ORDER BY p.key, q.number"#
    )
    .fetch_all(&mut *connection)
    .await?;
    let links = sqlx::query!(
        r#"SELECT qt.question_id AS "question_id: Uuid", qt.ticket_id AS "ticket_id: Uuid"
           FROM question_tickets qt
           JOIN questions q ON q.id = qt.question_id
           JOIN tickets t ON t.id = qt.ticket_id
           WHERE q.state = 'open'
           ORDER BY t.number"#
    )
    .fetch_all(connection)
    .await?;
    let mut tickets: HashMap<Uuid, Vec<Id<nullradix_domain::Ticket>>> = HashMap::new();
    for link in links {
        tickets
            .entry(link.question_id)
            .or_default()
            .push(Id::from_uuid(link.ticket_id));
    }
    rows.into_iter()
        .map(|row| {
            Ok(InboxQuestion {
                key: QuestionKey::new(project_key(&row.project_key)?, number(row.number)?),
                title: row.title,
                occasion: row.occasion,
                created_at: Timestamp::from_millis(row.created_at),
                tickets: tickets.remove(&row.id).unwrap_or_default(),
            })
        })
        .collect()
}

async fn notes(connection: &mut SqliteConnection) -> Result<Vec<InboxNote>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT n.rid AS "number!: i64", n.board_id AS "board_id: Uuid", n.title, n.body,
                  n.created_at,
                  (SELECT count(*) FROM note_sightings s WHERE s.note_id = n.id) AS "sightings!: i64"
           FROM notes n
           WHERE n.state = 'open'
           ORDER BY n.rid"#
    )
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| InboxNote {
            number: row.number,
            board: Id::from_uuid(row.board_id),
            title: row.title,
            body: row.body,
            seen_count: 1 + usize::try_from(row.sightings).unwrap_or_default(),
            created_at: Timestamp::from_millis(row.created_at),
        })
        .collect())
}

/// The saved order as keys; entries whose item is gone are skipped.
async fn saved_order(connection: &mut SqliteConnection) -> Result<Vec<InboxRef>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT tp.key AS "ticket_project?: String", t.number AS "ticket_number?: i64",
                  qp.key AS "question_project?: String", q.number AS "question_number?: i64",
                  n.rid AS "note_number?: i64"
           FROM inbox_order o
           LEFT JOIN tickets t ON o.item_type = 'ticket' AND t.id = o.item_id
           LEFT JOIN projects tp ON tp.id = t.project_id
           LEFT JOIN questions q ON o.item_type = 'question' AND q.id = o.item_id
           LEFT JOIN projects qp ON qp.id = q.project_id
           LEFT JOIN notes n ON o.item_type = 'note' AND n.id = o.item_id
           ORDER BY o.position"#
    )
    .fetch_all(connection)
    .await?;
    let mut order = Vec::with_capacity(rows.len());
    for row in rows {
        let entry = match (row.ticket_project, row.ticket_number) {
            (Some(key), Some(at)) => Some(InboxRef::Ticket(TicketKey::new(
                project_key(&key)?,
                number(at)?,
            ))),
            _ => match (row.question_project, row.question_number) {
                (Some(key), Some(at)) => Some(InboxRef::Question(QuestionKey::new(
                    project_key(&key)?,
                    number(at)?,
                ))),
                _ => row.note_number.map(InboxRef::Note),
            },
        };
        order.extend(entry);
    }
    Ok(order)
}

/// Id of the ticket, question or note an entry names, if it exists.
async fn item_id(
    connection: &mut SqliteConnection,
    entry: &InboxRef,
) -> Result<Option<Uuid>, sqlx::Error> {
    match entry {
        InboxRef::Ticket(key) => {
            let project = key.project().as_str();
            let number = key.number();
            sqlx::query_scalar!(
                r#"SELECT t.id AS "id: Uuid" FROM tickets t
                   JOIN projects p ON p.id = t.project_id
                   WHERE p.key = ? AND t.number = ?"#,
                project,
                number,
            )
            .fetch_optional(connection)
            .await
        }
        InboxRef::Question(key) => {
            let project = key.project().as_str();
            let number = key.number();
            sqlx::query_scalar!(
                r#"SELECT q.id AS "id: Uuid" FROM questions q
                   JOIN projects p ON p.id = q.project_id
                   WHERE p.key = ? AND q.number = ?"#,
                project,
                number,
            )
            .fetch_optional(connection)
            .await
        }
        InboxRef::Note(number) => {
            sqlx::query_scalar!(
                r#"SELECT id AS "id: Uuid" FROM notes WHERE rid = ?"#,
                number
            )
            .fetch_optional(connection)
            .await
        }
    }
}

#[async_trait]
impl InboxRepository for Store {
    async fn snapshot(&self) -> Result<InboxSnapshot, AppError> {
        // one read transaction keeps every list on the same snapshot
        let mut read = self.reader.begin().await.map_err(database)?;
        let snapshot = InboxSnapshot {
            boards: boards(&mut read).await?,
            tickets: tickets(&mut read).await?,
            questions: questions(&mut read).await?,
            notes: notes(&mut read).await?,
            order: saved_order(&mut read).await?,
        };
        read.commit().await.map_err(database)?;
        Ok(snapshot)
    }

    async fn save_order(&self, order: &[InboxRef]) -> Result<Vec<InboxRef>, AppError> {
        let mut write = self.begin().await?;
        sqlx::query!("DELETE FROM inbox_order")
            .execute(&mut *write)
            .await
            .map_err(database)?;
        let mut saved = Vec::with_capacity(order.len());
        for entry in order {
            let Some(id) = item_id(&mut write, entry).await.map_err(database)? else {
                continue;
            };
            let position = i64::try_from(saved.len()).unwrap_or(i64::MAX);
            let item_type = entry.type_name();
            sqlx::query!(
                "INSERT INTO inbox_order (position, item_type, item_id) VALUES (?, ?, ?)",
                position,
                item_type,
                id,
            )
            .execute(&mut *write)
            .await
            .map_err(database)?;
            saved.push(entry.clone());
        }
        write.commit().await.map_err(database)?;
        Ok(saved)
    }

    async fn clear_order(&self) -> Result<(), AppError> {
        sqlx::query!("DELETE FROM inbox_order")
            .execute(&self.writer)
            .await
            .map_err(database)?;
        Ok(())
    }
}
