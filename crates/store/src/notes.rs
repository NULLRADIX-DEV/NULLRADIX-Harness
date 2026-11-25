use std::collections::HashMap;

use async_trait::async_trait;
use nullradix_app::notes::{self, NoteRepository, NoteRow, NoteWrite};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    Board, Id, Note, NoteState, Project, Sighting, TicketKey, TicketRef, Timestamp, Version,
};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::error::{database, missed_guard};
use crate::tickets::Write;
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid {what}"))
}

fn ticket_ref(id: Option<Uuid>, key: Option<String>) -> Result<Option<TicketRef>, StoreError> {
    let Some(key) = key else {
        return Ok(None);
    };
    let key = TicketKey::parse(&key).map_err(|_| corrupt("ticket key"))?;
    Ok(Some(TicketRef {
        id: id.map(Id::from_uuid),
        key,
    }))
}

fn split(ticket: Option<&TicketRef>) -> (Option<Uuid>, Option<String>) {
    ticket.map_or((None, None), |ticket| {
        (
            ticket.id.map(|id| id.as_uuid()),
            Some(ticket.key.to_string()),
        )
    })
}

struct Row {
    number: i64,
    id: Uuid,
    board_id: Uuid,
    author_id: Uuid,
    author_name: String,
    title: String,
    body: String,
    source_ticket_id: Option<Uuid>,
    source_ticket_key: Option<String>,
    state: String,
    promoted_ticket_id: Option<Uuid>,
    promoted_ticket_key: Option<String>,
    reason: Option<String>,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

/// Labels and sightings of the read notes, by note id.
#[derive(Default)]
struct Lists {
    labels: HashMap<Uuid, Vec<String>>,
    sightings: HashMap<Uuid, Vec<Sighting>>,
}

impl Row {
    fn into_note_row(self, lists: &mut Lists) -> Result<NoteRow, StoreError> {
        let note = Note {
            id: Id::from_uuid(self.id),
            number: self.number,
            board: Id::from_uuid(self.board_id),
            author: Id::from_uuid(self.author_id),
            title: self.title,
            body: self.body,
            source: ticket_ref(self.source_ticket_id, self.source_ticket_key)?,
            labels: lists.labels.remove(&self.id).unwrap_or_default(),
            sightings: lists.sightings.remove(&self.id).unwrap_or_default(),
            state: NoteState::parse(&self.state).map_err(|_| corrupt("note state"))?,
            promoted_to: ticket_ref(self.promoted_ticket_id, self.promoted_ticket_key)?,
            reason: self.reason,
            created_at: Timestamp::from_millis(self.created_at),
            updated_at: Timestamp::from_millis(self.updated_at),
            version: Version::from_stored(self.version),
        };
        Ok(NoteRow {
            note,
            author_name: self.author_name,
        })
    }
}

/// Which notes a read covers; `None` does not narrow.
#[derive(Clone, Copy, Default)]
struct Selection {
    board: Option<Uuid>,
    number: Option<i64>,
}

async fn rows(connection: &mut SqliteConnection, at: Selection) -> Result<Vec<Row>, StoreError> {
    Ok(sqlx::query_as!(
        Row,
        r#"SELECT n.rid AS "number!: i64", n.id AS "id: Uuid", n.board_id AS "board_id: Uuid",
                  n.author_id AS "author_id: Uuid", a.name AS author_name, n.title, n.body,
                  n.source_ticket_id AS "source_ticket_id?: Uuid", n.source_ticket_key,
                  n.state, n.promoted_ticket_id AS "promoted_ticket_id?: Uuid",
                  n.promoted_ticket_key, n.reason, n.created_at, n.updated_at, n.version
           FROM notes n
           JOIN actors a ON a.id = n.author_id
           WHERE (?1 IS NULL OR n.board_id = ?1) AND (?2 IS NULL OR n.rid = ?2)
           ORDER BY n.rid"#,
        at.board,
        at.number,
    )
    .fetch_all(connection)
    .await?)
}

async fn lists(connection: &mut SqliteConnection, at: Selection) -> Result<Lists, StoreError> {
    let labels = sqlx::query!(
        r#"SELECT l.note_id AS "note_id: Uuid", l.label
           FROM note_labels l
           JOIN notes n ON n.id = l.note_id
           WHERE (?1 IS NULL OR n.board_id = ?1) AND (?2 IS NULL OR n.rid = ?2)
           ORDER BY l.note_id, l.position"#,
        at.board,
        at.number,
    )
    .fetch_all(&mut *connection)
    .await?;
    let sightings = sqlx::query!(
        r#"SELECT s.note_id AS "note_id: Uuid", s.ticket_id AS "ticket_id?: Uuid",
                  s.ticket_key, s.seen_at
           FROM note_sightings s
           JOIN notes n ON n.id = s.note_id
           WHERE (?1 IS NULL OR n.board_id = ?1) AND (?2 IS NULL OR n.rid = ?2)
           ORDER BY s.note_id, s.position"#,
        at.board,
        at.number,
    )
    .fetch_all(connection)
    .await?;
    let mut lists = Lists::default();
    for row in labels {
        lists.labels.entry(row.note_id).or_default().push(row.label);
    }
    for row in sightings {
        let sighting = Sighting {
            ticket: ticket_ref(row.ticket_id, row.ticket_key)?,
            at: Timestamp::from_millis(row.seen_at),
        };
        lists
            .sightings
            .entry(row.note_id)
            .or_default()
            .push(sighting);
    }
    Ok(lists)
}

async fn note_rows(
    connection: &mut SqliteConnection,
    at: Selection,
) -> Result<Vec<NoteRow>, StoreError> {
    let rows = rows(connection, at).await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut lists = lists(connection, at).await?;
    rows.into_iter()
        .map(|row| row.into_note_row(&mut lists))
        .collect()
}

async fn insert_lists(connection: &mut SqliteConnection, note: &Note) -> Result<(), AppError> {
    let id = note.id.as_uuid();
    for (position, label) in (0_i64..).zip(&note.labels) {
        sqlx::query!(
            "INSERT INTO note_labels (note_id, label, position) VALUES (?, ?, ?)",
            id,
            label,
            position,
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    }
    for (position, sighting) in (0_i64..).zip(&note.sightings) {
        let (ticket_id, ticket_key) = split(sighting.ticket.as_ref());
        let seen_at = sighting.at.as_millis();
        sqlx::query!(
            "INSERT INTO note_sightings (note_id, position, ticket_id, ticket_key, seen_at)
             VALUES (?, ?, ?, ?, ?)",
            id,
            position,
            ticket_id,
            ticket_key,
            seen_at,
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    }
    Ok(())
}

#[async_trait]
impl NoteRepository for Store {
    async fn begin(&self) -> Result<Box<dyn NoteWrite>, AppError> {
        Ok(Box::new(Write(self.begin().await?)))
    }

    async fn list(
        &self,
        project: Id<Project>,
        board: Id<Board>,
        states: &[NoteState],
        source: Option<&TicketKey>,
    ) -> Result<Option<Vec<NoteRow>>, AppError> {
        let board = board.as_uuid();
        let project = project.as_uuid();
        // one read transaction keeps the board check, rows and lists on the same snapshot
        let mut read = self.reader.begin().await.map_err(database)?;
        let found = sqlx::query_scalar!(
            "SELECT 1 FROM boards WHERE id = ? AND project_id = ?",
            board,
            project,
        )
        .fetch_optional(&mut *read)
        .await
        .map_err(database)?;
        if found.is_none() {
            return Ok(None);
        }
        let at = Selection {
            board: Some(board),
            number: None,
        };
        let rows = note_rows(&mut read, at).await?;
        read.commit().await.map_err(database)?;
        Ok(Some(
            rows.into_iter()
                .filter(|row| states.contains(&row.note.state))
                .filter(|row| {
                    source.is_none_or(|wanted| {
                        row.note
                            .source
                            .as_ref()
                            .is_some_and(|source| &source.key == wanted)
                    })
                })
                .collect(),
        ))
    }

    async fn get(&self, number: i64) -> Result<Option<NoteRow>, AppError> {
        let mut read = self.reader.begin().await.map_err(database)?;
        let at = Selection {
            board: None,
            number: Some(number),
        };
        let row = note_rows(&mut read, at).await?.pop();
        read.commit().await.map_err(database)?;
        Ok(row)
    }
}

#[async_trait]
impl NoteWrite for Write {
    async fn board_notes(
        &mut self,
        board: Id<Board>,
        states: &[NoteState],
    ) -> Result<Vec<Note>, AppError> {
        let at = Selection {
            board: Some(board.as_uuid()),
            number: None,
        };
        Ok(note_rows(&mut self.0, at)
            .await?
            .into_iter()
            .map(|row| row.note)
            .filter(|note| states.contains(&note.state))
            .collect())
    }

    async fn note(&mut self, number: i64) -> Result<Option<NoteRow>, AppError> {
        let at = Selection {
            board: None,
            number: Some(number),
        };
        Ok(note_rows(&mut self.0, at).await?.pop())
    }

    async fn insert_note(&mut self, note: &Note) -> Result<(i64, Change), AppError> {
        let id = note.id.as_uuid();
        let board = note.board.as_uuid();
        let author = note.author.as_uuid();
        let (source_id, source_key) = split(note.source.as_ref());
        let state = note.state.as_str();
        let (promoted_id, promoted_key) = split(note.promoted_to.as_ref());
        let created_at = note.created_at.as_millis();
        let updated_at = note.updated_at.as_millis();
        let version = note.version.value();
        let number = sqlx::query_scalar!(
            r#"INSERT INTO notes (id, board_id, author_id, title, body, source_ticket_id,
                                 source_ticket_key, state, promoted_ticket_id,
                                 promoted_ticket_key, reason, created_at, updated_at, version)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               RETURNING rid AS "rid!: i64""#,
            id,
            board,
            author,
            note.title,
            note.body,
            source_id,
            source_key,
            state,
            promoted_id,
            promoted_key,
            note.reason,
            created_at,
            updated_at,
            version,
        )
        .fetch_one(&mut *self.0)
        .await
        .map_err(database)?;
        insert_lists(&mut self.0, note).await?;
        let change = change_feed::record(
            &mut self.0,
            Entity::Note,
            id,
            ChangeOp::Insert,
            note.created_at,
        )
        .await?;
        Ok((number, change))
    }

    async fn update_note(&mut self, note: &Note, expected: Version) -> Result<Change, AppError> {
        let id = note.id.as_uuid();
        let (source_id, source_key) = split(note.source.as_ref());
        let state = note.state.as_str();
        let (promoted_id, promoted_key) = split(note.promoted_to.as_ref());
        let updated_at = note.updated_at.as_millis();
        let version = note.version.value();
        let expected = expected.value();
        let updated = sqlx::query!(
            "UPDATE notes SET title = ?, body = ?, source_ticket_id = ?, source_ticket_key = ?,
                              state = ?, promoted_ticket_id = ?, promoted_ticket_key = ?,
                              reason = ?, updated_at = ?, version = ?
             WHERE id = ? AND version = ?",
            note.title,
            note.body,
            source_id,
            source_key,
            state,
            promoted_id,
            promoted_key,
            note.reason,
            updated_at,
            version,
            id,
            expected,
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        if updated.rows_affected() == 0 {
            let exists = sqlx::query_scalar!("SELECT 1 FROM notes WHERE id = ?", id)
                .fetch_optional(&mut *self.0)
                .await;
            return Err(missed_guard(
                exists,
                AppError::version_conflict("note", note.number),
                notes::not_found(note.number),
            ));
        }
        sqlx::query!("DELETE FROM note_labels WHERE note_id = ?", id)
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
        sqlx::query!("DELETE FROM note_sightings WHERE note_id = ?", id)
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
        insert_lists(&mut self.0, note).await?;
        Ok(change_feed::record(
            &mut self.0,
            Entity::Note,
            id,
            ChangeOp::Update,
            note.updated_at,
        )
        .await?)
    }
}
