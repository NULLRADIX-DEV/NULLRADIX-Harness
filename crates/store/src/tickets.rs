use std::collections::HashMap;

use async_trait::async_trait;
use nullradix_app::tickets::{
    HistoryLine, StepProgress, TicketRead, TicketRepository, TicketRow, TicketScope,
    TicketSnapshot, TicketWrite,
};
use nullradix_app::{AppError, Change, ChangeOp, Entity, boards, projects, tickets};
use nullradix_domain::{
    Board, BoardName, BoardRecord, DependencyIndex, HistoryEntry, Id, Priority, Project,
    ProjectKey, Step, TargetBranch, Ticket, TicketKey, TicketRecord, TicketStatus, TicketTitle,
    Timestamp, Tombstone, Version,
};
use sqlx::{Sqlite, SqliteConnection, Transaction};
use uuid::Uuid;

use crate::boards::missed;
use crate::error::{database, missed_guard};
use crate::{Store, StoreError, change_feed, comments};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid {what}"))
}

fn ticket_key(project: &str, number: i64) -> Result<TicketKey, StoreError> {
    let project = ProjectKey::parse(project).map_err(|_| corrupt("project key"))?;
    let number = u32::try_from(number).map_err(|_| corrupt("ticket number"))?;
    Ok(TicketKey::new(project, number))
}

fn status(text: &str) -> Result<TicketStatus, StoreError> {
    text.parse().map_err(|_| corrupt("ticket status"))
}

struct Row {
    id: Uuid,
    project_key: String,
    project_id: Uuid,
    board_id: Uuid,
    number: i64,
    title: String,
    description: String,
    acceptance: String,
    plan: String,
    handbook: String,
    diagram: String,
    status: String,
    priority: String,
    sort_order: i64,
    assignee: Option<String>,
    needs_planning: i64,
    blocked_reason: Option<String>,
    target_branch: Option<String>,
    branch: Option<String>,
    worktree: Option<String>,
    planned_by: Option<String>,
    planned_at: Option<i64>,
    planning_claim_by: Option<String>,
    planning_claim_at: Option<i64>,
    created_by: Uuid,
    created_by_name: String,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

/// Labels and dependency keys of the read tickets, by ticket id.
#[derive(Default)]
struct Lists {
    labels: HashMap<Uuid, Vec<String>>,
    depends_on: HashMap<Uuid, Vec<TicketKey>>,
    progress: HashMap<Uuid, StepProgress>,
}

impl Row {
    fn into_ticket_row(self, lists: &mut Lists) -> Result<TicketRow, StoreError> {
        let target_branch = match self.target_branch.as_deref() {
            Some(name) => TargetBranch::parse(name).map_err(|_| corrupt("target branch"))?,
            None => None,
        };
        let record = TicketRecord {
            id: Id::from_uuid(self.id),
            key: ticket_key(&self.project_key, self.number)?,
            project: Id::from_uuid(self.project_id),
            board: Id::from_uuid(self.board_id),
            title: TicketTitle::parse(&self.title).map_err(|_| corrupt("ticket title"))?,
            description: self.description,
            acceptance: self.acceptance,
            plan: self.plan,
            handbook: self.handbook,
            diagram: self.diagram,
            status: status(&self.status)?,
            priority: Priority::parse(&self.priority).map_err(|_| corrupt("priority"))?,
            labels: lists.labels.remove(&self.id).unwrap_or_default(),
            depends_on: lists.depends_on.remove(&self.id).unwrap_or_default(),
            sort_order: self.sort_order,
            assignee: self.assignee,
            needs_planning: self.needs_planning != 0,
            blocked_reason: self.blocked_reason,
            target_branch,
            branch: self.branch,
            worktree: self.worktree,
            planned: stamp(self.planned_by, self.planned_at, "plan")?,
            planning_claim: stamp(
                self.planning_claim_by,
                self.planning_claim_at,
                "planning claim",
            )?,
            created_by: Id::from_uuid(self.created_by),
            created_at: Timestamp::from_millis(self.created_at),
            updated_at: Timestamp::from_millis(self.updated_at),
            version: Version::from_stored(self.version),
        };
        Ok(TicketRow {
            steps: lists.progress.remove(&self.id).unwrap_or_default(),
            ticket: Ticket::restore(record),
            created_by_name: self.created_by_name,
        })
    }
}

/// A name with its time, stored in two columns that are set or empty together.
fn stamp(
    name: Option<String>,
    at: Option<i64>,
    what: &str,
) -> Result<Option<(String, Timestamp)>, StoreError> {
    match (name, at) {
        (Some(name), Some(at)) => Ok(Some((name, Timestamp::from_millis(at)))),
        (None, None) => Ok(None),
        _ => Err(corrupt(what)),
    }
}

fn split(stamp: Option<&(String, Timestamp)>) -> (Option<&str>, Option<i64>) {
    stamp.map_or((None, None), |(name, at)| {
        (Some(name.as_str()), Some(at.as_millis()))
    })
}

/// Which tickets a read covers; `None` does not narrow.
#[derive(Clone, Copy, Default)]
struct Selection {
    project: Option<Uuid>,
    board: Option<Uuid>,
    ticket: Option<Uuid>,
}

async fn rows(connection: &mut SqliteConnection, at: Selection) -> Result<Vec<Row>, StoreError> {
    Ok(sqlx::query_as!(
        Row,
        r#"SELECT t.id AS "id: Uuid", p.key AS project_key, t.project_id AS "project_id: Uuid",
                  t.board_id AS "board_id: Uuid", t.number, t.title, t.description, t.acceptance,
                  t.plan, t.handbook, t.diagram, t.status, t.priority, t.sort_order, t.assignee,
                  t.needs_planning, t.blocked_reason, t.target_branch,
                  w.branch AS "branch?", w.path AS "worktree?", t.planned_by,
                  t.planned_at, t.planning_claim_by, t.planning_claim_at,
                  t.created_by AS "created_by: Uuid", a.name AS created_by_name, t.created_at,
                  t.updated_at, t.version
           FROM tickets t
           JOIN projects p ON p.id = t.project_id
           JOIN actors a ON a.id = t.created_by
           LEFT JOIN worktree_tickets l ON l.ticket_id = t.id AND l.released_at IS NULL
           LEFT JOIN worktrees w ON w.id = l.worktree_id
           WHERE (?1 IS NULL OR t.project_id = ?1) AND (?2 IS NULL OR t.board_id = ?2)
             AND (?3 IS NULL OR t.id = ?3)"#,
        at.project,
        at.board,
        at.ticket,
    )
    .fetch_all(connection)
    .await?)
}

async fn labels(
    connection: &mut SqliteConnection,
    at: Selection,
) -> Result<HashMap<Uuid, Vec<String>>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT l.ticket_id AS "ticket_id: Uuid", l.label
           FROM ticket_labels l
           JOIN tickets t ON t.id = l.ticket_id
           WHERE (?1 IS NULL OR t.project_id = ?1) AND (?2 IS NULL OR t.board_id = ?2)
             AND (?3 IS NULL OR t.id = ?3)
           ORDER BY l.ticket_id, l.position"#,
        at.project,
        at.board,
        at.ticket,
    )
    .fetch_all(connection)
    .await?;
    let mut labels: HashMap<Uuid, Vec<String>> = HashMap::new();
    for row in rows {
        labels.entry(row.ticket_id).or_default().push(row.label);
    }
    Ok(labels)
}

async fn dependencies(
    connection: &mut SqliteConnection,
    at: Selection,
) -> Result<HashMap<Uuid, Vec<TicketKey>>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT e.ticket_id AS "ticket_id: Uuid", dp.key AS project_key, d.number
           FROM ticket_dependencies e
           JOIN tickets t ON t.id = e.ticket_id
           JOIN tickets d ON d.id = e.depends_on_id
           JOIN projects dp ON dp.id = d.project_id
           WHERE (?1 IS NULL OR t.project_id = ?1) AND (?2 IS NULL OR t.board_id = ?2)
             AND (?3 IS NULL OR t.id = ?3)
           ORDER BY e.ticket_id, e.position"#,
        at.project,
        at.board,
        at.ticket,
    )
    .fetch_all(connection)
    .await?;
    let mut dependencies: HashMap<Uuid, Vec<TicketKey>> = HashMap::new();
    for row in rows {
        dependencies
            .entry(row.ticket_id)
            .or_default()
            .push(ticket_key(&row.project_key, row.number)?);
    }
    Ok(dependencies)
}

async fn progress(
    connection: &mut SqliteConnection,
    at: Selection,
) -> Result<HashMap<Uuid, StepProgress>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT s.ticket_id AS "ticket_id: Uuid", count(*) AS "total!: i64",
                  sum(s.done) AS "done!: i64"
           FROM ticket_steps s
           JOIN tickets t ON t.id = s.ticket_id
           WHERE (?1 IS NULL OR t.project_id = ?1) AND (?2 IS NULL OR t.board_id = ?2)
             AND (?3 IS NULL OR t.id = ?3)
           GROUP BY s.ticket_id"#,
        at.project,
        at.board,
        at.ticket,
    )
    .fetch_all(connection)
    .await?;
    let count = |value: i64| u32::try_from(value).map_err(|_| corrupt("step count"));
    rows.into_iter()
        .map(|row| {
            let progress = StepProgress {
                done: count(row.done)?,
                total: count(row.total)?,
            };
            Ok((row.ticket_id, progress))
        })
        .collect()
}

async fn steps(connection: &mut SqliteConnection, ticket: Uuid) -> Result<Vec<Step>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id: Uuid", text, done, detail FROM ticket_steps
           WHERE ticket_id = ? ORDER BY position"#,
        ticket,
    )
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Step {
            id: Id::from_uuid(row.id),
            text: row.text,
            done: row.done != 0,
            detail: row.detail,
        })
        .collect())
}

async fn ticket_rows(
    connection: &mut SqliteConnection,
    at: Selection,
) -> Result<Vec<TicketRow>, StoreError> {
    let rows = rows(&mut *connection, at).await?;
    let mut lists = Lists {
        labels: labels(&mut *connection, at).await?,
        depends_on: dependencies(&mut *connection, at).await?,
        progress: progress(connection, at).await?,
    };
    rows.into_iter()
        .map(|row| row.into_ticket_row(&mut lists))
        .collect()
}

async fn dependency_index(
    connection: &mut SqliteConnection,
) -> Result<DependencyIndex, StoreError> {
    let tickets = sqlx::query!(
        r#"SELECT t.id AS "id: Uuid", p.key AS project_key, t.number, t.status
           FROM tickets t JOIN projects p ON p.id = t.project_id"#
    )
    .fetch_all(&mut *connection)
    .await?;
    let mut depends_on = dependencies(connection, Selection::default()).await?;
    let mut nodes = Vec::with_capacity(tickets.len());
    for ticket in tickets {
        nodes.push((
            ticket_key(&ticket.project_key, ticket.number)?,
            status(&ticket.status)?,
            depends_on.remove(&ticket.id).unwrap_or_default(),
        ));
    }
    Ok(DependencyIndex::new(nodes))
}

/// The id of the ticket with this key.
pub(crate) async fn ticket_id(
    connection: &mut SqliteConnection,
    key: &TicketKey,
) -> Result<Option<Uuid>, StoreError> {
    let project = key.project().as_str();
    let number = i64::from(key.number());
    Ok(sqlx::query_scalar!(
        r#"SELECT t.id AS "id: Uuid" FROM tickets t JOIN projects p ON p.id = t.project_id
           WHERE p.key = ? AND t.number = ?"#,
        project,
        number,
    )
    .fetch_optional(connection)
    .await?)
}

async fn history(
    connection: &mut SqliteConnection,
    ticket: Uuid,
) -> Result<Vec<HistoryLine>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT h.at, h.actor_id AS "actor_id: Uuid", a.name AS actor_name, h.field,
                  h.from_value, h.to_value, h.note
           FROM activity h JOIN actors a ON a.id = h.actor_id
           WHERE h.ticket_id = ?
           ORDER BY h.at, h.id"#,
        ticket,
    )
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| HistoryLine {
            at: Timestamp::from_millis(row.at),
            actor: Id::from_uuid(row.actor_id),
            actor_name: row.actor_name,
            field: row.field,
            from: row.from_value,
            to: row.to_value,
            note: row.note,
        })
        .collect())
}

#[async_trait]
impl TicketRepository for Store {
    async fn begin(&self) -> Result<Box<dyn TicketWrite>, AppError> {
        Ok(Box::new(Write(self.begin().await?)))
    }

    async fn list(&self, scope: TicketScope) -> Result<TicketSnapshot, AppError> {
        let at = Selection {
            project: scope.project.map(|id| id.as_uuid()),
            board: scope.board.map(|id| id.as_uuid()),
            ticket: None,
        };
        // one read transaction keeps rows, lists and the index on the same snapshot
        let mut read = self.reader.begin().await.map_err(database)?;
        let rows = ticket_rows(&mut read, at).await?;
        if rows.is_empty() {
            missing_scope(&mut read, scope).await?;
        }
        let index = dependency_index(&mut read).await?;
        read.commit().await.map_err(database)?;
        Ok(TicketSnapshot { rows, index })
    }

    async fn get(&self, key: &TicketKey) -> Result<Option<TicketRead>, AppError> {
        let mut read = self.reader.begin().await.map_err(database)?;
        let Some(id) = ticket_id(&mut read, key).await? else {
            return Ok(None);
        };
        let at = Selection {
            ticket: Some(id),
            ..Selection::default()
        };
        let Some(row) = ticket_rows(&mut read, at).await?.pop() else {
            return Ok(None);
        };
        let index = dependency_index(&mut read).await?;
        let history = history(&mut read, id).await?;
        let steps = steps(&mut read, id).await?;
        let comments = comments::ticket_comments(&mut read, id, false).await?;
        read.commit().await.map_err(database)?;
        Ok(Some(TicketRead {
            row,
            index,
            history,
            steps,
            comments,
        }))
    }
}

/// Fails when the scope names a project or board that does not exist.
async fn missing_scope(
    connection: &mut SqliteConnection,
    scope: TicketScope,
) -> Result<(), AppError> {
    if let Some(project) = scope.project {
        let uuid = project.as_uuid();
        let found = sqlx::query_scalar!("SELECT 1 FROM projects WHERE id = ?", uuid)
            .fetch_optional(&mut *connection)
            .await
            .map_err(database)?;
        if found.is_none() {
            return Err(projects::not_found(project));
        }
    }
    if let Some(board) = scope.board {
        let uuid = board.as_uuid();
        let found = sqlx::query_scalar!("SELECT 1 FROM boards WHERE id = ?", uuid)
            .fetch_optional(connection)
            .await
            .map_err(database)?;
        if found.is_none() {
            return Err(boards::not_found(board));
        }
    }
    Ok(())
}

pub(crate) struct Write(pub(crate) Transaction<'static, Sqlite>);

#[async_trait]
impl TicketWrite for Write {
    async fn board(&mut self, id: Id<Board>) -> Result<Option<(Board, ProjectKey)>, AppError> {
        let uuid = id.as_uuid();
        let Some(row) = sqlx::query!(
            r#"SELECT b.id AS "id: Uuid", b.project_id AS "project_id: Uuid", b.name,
                      b.owner_id AS "owner_id: Uuid", b.dispatch_enabled,
                      b.repository_id AS "repository_id?: Uuid", b.created_at,
                      b.updated_at, b.version, p.key AS project_key
               FROM boards b JOIN projects p ON p.id = b.project_id
               WHERE b.id = ?"#,
            uuid,
        )
        .fetch_optional(&mut *self.0)
        .await
        .map_err(database)?
        else {
            return Ok(None);
        };
        let board = Board::restore(BoardRecord {
            id: Id::from_uuid(row.id),
            project: Id::from_uuid(row.project_id),
            name: BoardName::parse(&row.name).map_err(|_| corrupt("board name"))?,
            owner: Id::from_uuid(row.owner_id),
            dispatch_enabled: row.dispatch_enabled != 0,
            repository: row.repository_id.map(Id::from_uuid),
            created_at: Timestamp::from_millis(row.created_at),
            updated_at: Timestamp::from_millis(row.updated_at),
            version: Version::from_stored(row.version),
        });
        let key = ProjectKey::parse(&row.project_key).map_err(|_| corrupt("project key"))?;
        Ok(Some((board, key)))
    }

    async fn open_titles(
        &mut self,
        board: Id<Board>,
    ) -> Result<Vec<(TicketKey, TicketTitle)>, AppError> {
        let uuid = board.as_uuid();
        let rows = sqlx::query!(
            r#"SELECT p.key AS project_key, t.number, t.title
               FROM tickets t JOIN projects p ON p.id = t.project_id
               WHERE t.board_id = ? AND t.status NOT IN ('done', 'not-needed')
               ORDER BY t.number"#,
            uuid,
        )
        .fetch_all(&mut *self.0)
        .await
        .map_err(database)?;
        rows.into_iter()
            .map(|row| {
                let key = ticket_key(&row.project_key, row.number)?;
                let title = TicketTitle::parse(&row.title).map_err(|_| corrupt("ticket title"))?;
                Ok((key, title))
            })
            .collect::<Result<_, StoreError>>()
            .map_err(AppError::from)
    }

    async fn dependency_index(&mut self) -> Result<DependencyIndex, AppError> {
        Ok(dependency_index(&mut self.0).await?)
    }

    async fn take_number(&mut self, project: Id<Project>) -> Result<u32, AppError> {
        let uuid = project.as_uuid();
        let number = sqlx::query_scalar!(
            r#"UPDATE projects SET next_ticket_number = next_ticket_number + 1 WHERE id = ?
               RETURNING next_ticket_number - 1 AS "number!: i64""#,
            uuid,
        )
        .fetch_optional(&mut *self.0)
        .await
        .map_err(database)?
        .ok_or_else(|| projects::not_found(project))?;
        Ok(u32::try_from(number).map_err(|_| corrupt("ticket counter"))?)
    }

    async fn next_sort_order(
        &mut self,
        board: Id<Board>,
        status: TicketStatus,
    ) -> Result<i64, AppError> {
        let uuid = board.as_uuid();
        let status = status.as_str();
        let last = sqlx::query_scalar!(
            r#"SELECT max(sort_order) AS "last: i64" FROM tickets
               WHERE board_id = ? AND status = ?"#,
            uuid,
            status,
        )
        .fetch_one(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(last.map_or(0, |last| last + 1))
    }

    async fn insert(
        &mut self,
        ticket: &Ticket,
        history: &[HistoryEntry],
    ) -> Result<Change, AppError> {
        let record = ticket.record();
        insert_ticket(&mut self.0, record).await?;
        insert_lists(&mut self.0, record).await?;
        insert_history(&mut self.0, history).await?;
        let change = change_feed::record(
            &mut self.0,
            Entity::Ticket,
            record.id.as_uuid(),
            ChangeOp::Insert,
            record.created_at,
        )
        .await?;
        Ok(change)
    }

    async fn ticket(&mut self, key: &TicketKey) -> Result<Option<Ticket>, AppError> {
        let Some(id) = ticket_id(&mut self.0, key).await? else {
            return Ok(None);
        };
        Ok(one(&mut self.0, id).await?)
    }

    async fn board_tickets(&mut self, board: Id<Board>) -> Result<Vec<Ticket>, AppError> {
        let at = Selection {
            board: Some(board.as_uuid()),
            ..Selection::default()
        };
        let mut tickets: Vec<Ticket> = ticket_rows(&mut self.0, at)
            .await?
            .into_iter()
            .map(|row| row.ticket)
            .collect();
        tickets.sort_by_key(|ticket| ticket.key().number());
        Ok(tickets)
    }

    async fn dependents(&mut self, ticket: Id<Ticket>) -> Result<Vec<Ticket>, AppError> {
        let uuid = ticket.as_uuid();
        let ids = sqlx::query_scalar!(
            r#"SELECT e.ticket_id AS "ticket_id: Uuid" FROM ticket_dependencies e
               JOIN tickets t ON t.id = e.ticket_id
               JOIN projects p ON p.id = t.project_id
               WHERE e.depends_on_id = ?
               ORDER BY p.key, t.number"#,
            uuid,
        )
        .fetch_all(&mut *self.0)
        .await
        .map_err(database)?;
        let mut dependents = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(ticket) = one(&mut self.0, id).await? {
                dependents.push(ticket);
            }
        }
        Ok(dependents)
    }

    async fn update(
        &mut self,
        ticket: &Ticket,
        expected: Version,
        history: &[HistoryEntry],
    ) -> Result<Change, AppError> {
        let record = ticket.record();
        update_ticket(&mut self.0, record, expected).await?;
        let id = record.id.as_uuid();
        sqlx::query!("DELETE FROM ticket_labels WHERE ticket_id = ?", id)
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
        sqlx::query!("DELETE FROM ticket_dependencies WHERE ticket_id = ?", id)
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
        insert_lists(&mut self.0, record).await?;
        insert_history(&mut self.0, history).await?;
        Ok(change_feed::record(
            &mut self.0,
            Entity::Ticket,
            id,
            ChangeOp::Update,
            record.updated_at,
        )
        .await?)
    }

    async fn delete(&mut self, ticket: &Ticket, tombstone: &Tombstone) -> Result<Change, AppError> {
        let id = ticket.id().as_uuid();
        let deleted = sqlx::query!("DELETE FROM tickets WHERE id = ?", id)
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
        if deleted.rows_affected() == 0 {
            return Err(tickets::not_found(ticket.key()));
        }
        let tombstone_id = tombstone.id.as_uuid();
        let project = tombstone.project.as_uuid();
        let key = tombstone.key.to_string();
        let status = tombstone.from_status.as_str();
        let actor = tombstone.actor.as_uuid();
        let at = tombstone.at.as_millis();
        sqlx::query!(
            "INSERT INTO deleted_items (id, entity, entity_id, project_id, human_key, title,
                                        from_status, actor_id, at)
             VALUES (?, 'ticket', ?, ?, ?, ?, ?, ?, ?)",
            tombstone_id,
            id,
            project,
            key,
            tombstone.title,
            status,
            actor,
            at,
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(change_feed::record(
            &mut self.0,
            Entity::Ticket,
            id,
            ChangeOp::Delete,
            tombstone.at,
        )
        .await?)
    }

    async fn delete_board(
        &mut self,
        board: Id<Board>,
        expected: Version,
        at: Timestamp,
    ) -> Result<Change, AppError> {
        let uuid = board.as_uuid();
        let version = expected.value();
        let deleted = sqlx::query!(
            "DELETE FROM boards WHERE id = ? AND version = ?",
            uuid,
            version,
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        if deleted.rows_affected() == 0 {
            return Err(missed(&mut self.0, board).await);
        }
        Ok(change_feed::record(&mut self.0, Entity::Board, uuid, ChangeOp::Delete, at).await?)
    }

    async fn steps(&mut self, ticket: Id<Ticket>) -> Result<Vec<Step>, AppError> {
        Ok(steps(&mut self.0, ticket.as_uuid()).await?)
    }

    async fn replace_steps(&mut self, ticket: Id<Ticket>, steps: &[Step]) -> Result<(), AppError> {
        let id = ticket.as_uuid();
        sqlx::query!("DELETE FROM ticket_steps WHERE ticket_id = ?", id)
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
        for (position, step) in (0_i64..).zip(steps) {
            let done = i64::from(step.done);
            let step_id = step.id.as_uuid();
            sqlx::query!(
                "INSERT INTO ticket_steps (id, ticket_id, position, text, done, detail)
                 VALUES (?, ?, ?, ?, ?, ?)",
                step_id,
                id,
                position,
                step.text,
                done,
                step.detail,
            )
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
        }
        Ok(())
    }

    async fn commit(self: Box<Self>) -> Result<(), AppError> {
        self.0.commit().await.map_err(database)
    }
}

async fn one(connection: &mut SqliteConnection, id: Uuid) -> Result<Option<Ticket>, StoreError> {
    let at = Selection {
        ticket: Some(id),
        ..Selection::default()
    };
    Ok(ticket_rows(connection, at)
        .await?
        .pop()
        .map(|row| row.ticket))
}

async fn update_ticket(
    connection: &mut SqliteConnection,
    record: &TicketRecord,
    expected: Version,
) -> Result<(), AppError> {
    let id = record.id.as_uuid();
    let board = record.board.as_uuid();
    let title = record.title.as_str();
    let status = record.status.as_str();
    let priority = record.priority.as_str();
    let needs_planning = i64::from(record.needs_planning);
    let target_branch = record.target_branch.as_ref().map(TargetBranch::as_str);
    let (planned_by, planned_at) = split(record.planned.as_ref());
    let (claim_by, claim_at) = split(record.planning_claim.as_ref());
    let updated_at = record.updated_at.as_millis();
    let version = record.version.value();
    let expected = expected.value();
    let updated = sqlx::query!(
        "UPDATE tickets SET board_id = ?, title = ?, description = ?, acceptance = ?, plan = ?,
                            handbook = ?, diagram = ?, status = ?, priority = ?, sort_order = ?,
                            assignee = ?, needs_planning = ?, blocked_reason = ?,
                            target_branch = ?, planned_by = ?, planned_at = ?,
                            planning_claim_by = ?, planning_claim_at = ?, updated_at = ?,
                            version = ?
         WHERE id = ? AND version = ?",
        board,
        title,
        record.description,
        record.acceptance,
        record.plan,
        record.handbook,
        record.diagram,
        status,
        priority,
        record.sort_order,
        record.assignee,
        needs_planning,
        record.blocked_reason,
        target_branch,
        planned_by,
        planned_at,
        claim_by,
        claim_at,
        updated_at,
        version,
        id,
        expected,
    )
    .execute(&mut *connection)
    .await
    .map_err(database)?;
    if updated.rows_affected() == 0 {
        let exists = sqlx::query_scalar!("SELECT 1 FROM tickets WHERE id = ?", id)
            .fetch_optional(connection)
            .await;
        return Err(missed_guard(
            exists,
            AppError::version_conflict("ticket", &record.key),
            tickets::not_found(&record.key),
        ));
    }
    Ok(())
}

async fn insert_ticket(
    connection: &mut SqliteConnection,
    record: &TicketRecord,
) -> Result<(), AppError> {
    let id = record.id.as_uuid();
    let project = record.project.as_uuid();
    let board = record.board.as_uuid();
    let number = i64::from(record.key.number());
    let title = record.title.as_str();
    let status = record.status.as_str();
    let priority = record.priority.as_str();
    let needs_planning = i64::from(record.needs_planning);
    let target_branch = record.target_branch.as_ref().map(TargetBranch::as_str);
    let created_by = record.created_by.as_uuid();
    let created_at = record.created_at.as_millis();
    let updated_at = record.updated_at.as_millis();
    let version = record.version.value();
    let inserted = sqlx::query!(
        "INSERT INTO tickets (id, project_id, board_id, number, title, description, acceptance,
                              plan, handbook, diagram, status, priority, sort_order, assignee,
                              needs_planning, blocked_reason, target_branch, created_by,
                              created_at, updated_at, version)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        id,
        project,
        board,
        number,
        title,
        record.description,
        record.acceptance,
        record.plan,
        record.handbook,
        record.diagram,
        status,
        priority,
        record.sort_order,
        record.assignee,
        needs_planning,
        record.blocked_reason,
        target_branch,
        created_by,
        created_at,
        updated_at,
        version,
    )
    .execute(connection)
    .await;
    inserted.map(|_| ()).map_err(database)
}

async fn insert_lists(
    connection: &mut SqliteConnection,
    record: &TicketRecord,
) -> Result<(), AppError> {
    let id = record.id.as_uuid();
    for (position, label) in (0_i64..).zip(&record.labels) {
        sqlx::query!(
            "INSERT INTO ticket_labels (ticket_id, label, position) VALUES (?, ?, ?)",
            id,
            label,
            position,
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    }
    for (position, dependency) in (0_i64..).zip(&record.depends_on) {
        let project = dependency.project().as_str();
        let number = i64::from(dependency.number());
        let added = sqlx::query!(
            "INSERT INTO ticket_dependencies (ticket_id, depends_on_id, position)
             SELECT ?, t.id, ? FROM tickets t JOIN projects p ON p.id = t.project_id
             WHERE p.key = ? AND t.number = ?",
            id,
            position,
            project,
            number,
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
        if added.rows_affected() == 0 {
            return Err(StoreError::Corrupt(format!("dependency {dependency} vanished")).into());
        }
    }
    Ok(())
}

async fn insert_history(
    connection: &mut SqliteConnection,
    history: &[HistoryEntry],
) -> Result<(), AppError> {
    for entry in history {
        let id = entry.id.as_uuid();
        let ticket = entry.ticket.as_uuid();
        let actor = entry.actor.as_uuid();
        let at = entry.at.as_millis();
        sqlx::query!(
            "INSERT INTO activity (id, ticket_id, actor_id, at, field, from_value, to_value, note)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            ticket,
            actor,
            at,
            entry.field,
            entry.from,
            entry.to,
            entry.note,
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    }
    Ok(())
}
