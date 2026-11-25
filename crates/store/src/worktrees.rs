use async_trait::async_trait;
use nullradix_app::worktrees::{
    WorktreeEntry, WorktreeRepository, WorktreeWrite, already_started, branch_exists, not_found,
};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    ActualState, DesiredState, Id, ProjectKey, SetupState, TargetBranch, Ticket, TicketKey,
    Timestamp, Version, Worktree, WorktreeBranch, WorktreePath, WorktreePurpose, WorktreeRecord,
    WorktreeTicket,
};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::error::{database, unique_violation};
use crate::tickets::Write;
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid {what}"))
}

struct Row {
    id: Uuid,
    repository_id: Uuid,
    runner_id: Uuid,
    branch: String,
    target_branch: Option<String>,
    path: String,
    purpose: String,
    desired_state: String,
    actual_state: String,
    setup_state: String,
    ready_to_merge: i64,
    last_error: Option<String>,
    observed_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

impl Row {
    fn into_worktree(self) -> Result<Worktree, StoreError> {
        let target_branch = match self.target_branch.as_deref() {
            Some(name) => TargetBranch::parse(name).map_err(|_| corrupt("target branch"))?,
            None => None,
        };
        Ok(Worktree::restore(WorktreeRecord {
            id: Id::from_uuid(self.id),
            repository: Id::from_uuid(self.repository_id),
            runner: Id::from_uuid(self.runner_id),
            branch: WorktreeBranch::parse(&self.branch).map_err(|_| corrupt("branch"))?,
            target_branch,
            path: WorktreePath::parse(&self.path).map_err(|_| corrupt("worktree path"))?,
            purpose: self
                .purpose
                .parse::<WorktreePurpose>()
                .map_err(|_| corrupt("worktree purpose"))?,
            desired: self
                .desired_state
                .parse::<DesiredState>()
                .map_err(|_| corrupt("desired state"))?,
            actual: self
                .actual_state
                .parse::<ActualState>()
                .map_err(|_| corrupt("actual state"))?,
            setup: self
                .setup_state
                .parse::<SetupState>()
                .map_err(|_| corrupt("setup state"))?,
            ready_to_merge: self.ready_to_merge != 0,
            last_error: self.last_error,
            observed_at: self.observed_at.map(Timestamp::from_millis),
            created_at: Timestamp::from_millis(self.created_at),
            updated_at: Timestamp::from_millis(self.updated_at),
            version: Version::from_stored(self.version),
        }))
    }
}

async fn insert(
    connection: &mut SqliteConnection,
    worktree: &Worktree,
    owner: Id<Ticket>,
) -> Result<Change, AppError> {
    let record = worktree.record();
    let id = record.id.as_uuid();
    let repository = record.repository.as_uuid();
    let runner = record.runner.as_uuid();
    let branch = record.branch.as_str();
    let target_branch = record.target_branch.as_ref().map(TargetBranch::as_str);
    let path = record.path.as_str();
    let purpose = record.purpose.as_str();
    let desired = record.desired.as_str();
    let actual = record.actual.as_str();
    let setup = record.setup.as_str();
    let ready = i64::from(record.ready_to_merge);
    let observed_at = record.observed_at.map(Timestamp::as_millis);
    let created_at = record.created_at.as_millis();
    let updated_at = record.updated_at.as_millis();
    let version = record.version.value();
    sqlx::query!(
        "INSERT INTO worktrees (id, repository_id, runner_id, branch, target_branch, path, purpose,
                                desired_state, actual_state, setup_state, ready_to_merge,
                                last_error, observed_at, created_at, updated_at, version)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        id,
        repository,
        runner,
        branch,
        target_branch,
        path,
        purpose,
        desired,
        actual,
        setup,
        ready,
        record.last_error,
        observed_at,
        created_at,
        updated_at,
        version,
    )
    .execute(&mut *connection)
    .await
    .map_err(|error| {
        if unique_violation(&error, "worktrees.branch") {
            branch_exists(&record.branch)
        } else {
            database(error)
        }
    })?;
    let ticket = owner.as_uuid();
    sqlx::query!(
        "INSERT INTO worktree_tickets (worktree_id, ticket_id, position, joined_at)
         VALUES (?, ?, 0, ?)",
        id,
        ticket,
        created_at,
    )
    .execute(&mut *connection)
    .await
    .map_err(|error| linked(error, owner))?;
    Ok(change_feed::record(
        connection,
        Entity::Worktree,
        id,
        ChangeOp::Insert,
        record.created_at,
    )
    .await?)
}

/// A link that breaks the one live link per ticket means the ticket is started already.
fn linked(error: sqlx::Error, ticket: Id<Ticket>) -> AppError {
    if unique_violation(&error, "worktree_tickets.ticket_id") {
        already_started(ticket)
    } else {
        database(error)
    }
}

async fn join(
    connection: &mut SqliteConnection,
    worktree: Id<Worktree>,
    ticket: Id<Ticket>,
    at: Timestamp,
) -> Result<Change, AppError> {
    let id = worktree.as_uuid();
    let found = sqlx::query_scalar!("SELECT 1 FROM worktrees WHERE id = ?", id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(database)?;
    if found.is_none() {
        return Err(not_found(worktree));
    }
    let uuid = ticket.as_uuid();
    let millis = at.as_millis();
    // legacy never listed a ticket twice: joining again keeps its place
    sqlx::query!(
        "INSERT INTO worktree_tickets (worktree_id, ticket_id, position, joined_at)
         SELECT ?1, ?2, coalesce(max(position) + 1, 0), ?3 FROM worktree_tickets
         WHERE worktree_id = ?1
         ON CONFLICT (worktree_id, ticket_id) DO UPDATE SET released_at = NULL",
        id,
        uuid,
        millis,
    )
    .execute(&mut *connection)
    .await
    .map_err(|error| linked(error, ticket))?;
    Ok(change_feed::record(connection, Entity::Worktree, id, ChangeOp::Update, at).await?)
}

async fn release(
    connection: &mut SqliteConnection,
    ticket: Id<Ticket>,
    at: Timestamp,
) -> Result<Option<Change>, AppError> {
    let uuid = ticket.as_uuid();
    let millis = at.as_millis();
    let released = sqlx::query_scalar!(
        r#"UPDATE worktree_tickets SET released_at = ?
           WHERE ticket_id = ? AND released_at IS NULL
           RETURNING worktree_id AS "worktree_id: Uuid""#,
        millis,
        uuid,
    )
    .fetch_optional(&mut *connection)
    .await
    .map_err(database)?;
    match released {
        Some(worktree) => Ok(Some(
            change_feed::record(connection, Entity::Worktree, worktree, ChangeOp::Update, at)
                .await?,
        )),
        None => Ok(None),
    }
}

async fn entry(
    connection: &mut SqliteConnection,
    id: Uuid,
) -> Result<Option<WorktreeEntry>, StoreError> {
    let Some(row) = sqlx::query_as!(
        Row,
        r#"SELECT id AS "id: Uuid", repository_id AS "repository_id: Uuid",
                  runner_id AS "runner_id: Uuid", branch, target_branch, path, purpose,
                  desired_state, actual_state, setup_state, ready_to_merge, last_error,
                  observed_at, created_at, updated_at, version
           FROM worktrees WHERE id = ?"#,
        id,
    )
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(None);
    };
    let links = sqlx::query!(
        r#"SELECT l.ticket_id AS "ticket_id: Uuid", p.key AS project_key, t.number, l.position,
                  l.joined_at, l.released_at
           FROM worktree_tickets l
           JOIN tickets t ON t.id = l.ticket_id
           JOIN projects p ON p.id = t.project_id
           WHERE l.worktree_id = ?
           ORDER BY l.position"#,
        id,
    )
    .fetch_all(connection)
    .await?;
    let tickets = links
        .into_iter()
        .map(|link| {
            let project =
                ProjectKey::parse(&link.project_key).map_err(|_| corrupt("project key"))?;
            let number = u32::try_from(link.number).map_err(|_| corrupt("ticket number"))?;
            Ok(WorktreeTicket {
                ticket: Id::from_uuid(link.ticket_id),
                key: TicketKey::new(project, number),
                position: u32::try_from(link.position).map_err(|_| corrupt("link position"))?,
                joined_at: Timestamp::from_millis(link.joined_at),
                released_at: link.released_at.map(Timestamp::from_millis),
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    Ok(Some(WorktreeEntry {
        worktree: row.into_worktree()?,
        tickets,
    }))
}

async fn live(
    connection: &mut SqliteConnection,
    ticket: Id<Ticket>,
) -> Result<Option<WorktreeEntry>, StoreError> {
    let uuid = ticket.as_uuid();
    let worktree = sqlx::query_scalar!(
        r#"SELECT worktree_id AS "worktree_id: Uuid" FROM worktree_tickets
           WHERE ticket_id = ? AND released_at IS NULL"#,
        uuid,
    )
    .fetch_optional(&mut *connection)
    .await?;
    match worktree {
        Some(id) => entry(connection, id).await,
        None => Ok(None),
    }
}

#[async_trait]
impl WorktreeWrite for Write {
    async fn insert_worktree(
        &mut self,
        worktree: &Worktree,
        owner: Id<Ticket>,
    ) -> Result<Change, AppError> {
        insert(&mut self.0, worktree, owner).await
    }

    async fn join_worktree(
        &mut self,
        worktree: Id<Worktree>,
        ticket: Id<Ticket>,
        at: Timestamp,
    ) -> Result<Change, AppError> {
        join(&mut self.0, worktree, ticket, at).await
    }

    async fn release_link(
        &mut self,
        ticket: Id<Ticket>,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        release(&mut self.0, ticket, at).await
    }

    async fn live_worktree(
        &mut self,
        ticket: Id<Ticket>,
    ) -> Result<Option<WorktreeEntry>, AppError> {
        Ok(live(&mut self.0, ticket).await?)
    }
}

#[async_trait]
impl WorktreeRepository for Store {
    async fn insert(&self, worktree: &Worktree, owner: Id<Ticket>) -> Result<Change, AppError> {
        let mut tx = self.begin().await?;
        let change = insert(&mut tx, worktree, owner).await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn join(
        &self,
        worktree: Id<Worktree>,
        ticket: Id<Ticket>,
        at: Timestamp,
    ) -> Result<Change, AppError> {
        let mut tx = self.begin().await?;
        let change = join(&mut tx, worktree, ticket, at).await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn release(&self, ticket: Id<Ticket>, at: Timestamp) -> Result<Option<Change>, AppError> {
        let mut tx = self.begin().await?;
        let change = release(&mut tx, ticket, at).await?;
        tx.commit().await.map_err(database)?;
        Ok(change)
    }

    async fn get(&self, id: Id<Worktree>) -> Result<Option<WorktreeEntry>, AppError> {
        let mut read = self.reader.begin().await.map_err(database)?;
        let found = entry(&mut read, id.as_uuid()).await?;
        read.commit().await.map_err(database)?;
        Ok(found)
    }

    async fn of_ticket(&self, ticket: Id<Ticket>) -> Result<Option<WorktreeEntry>, AppError> {
        let mut read = self.reader.begin().await.map_err(database)?;
        let found = live(&mut read, ticket).await?;
        read.commit().await.map_err(database)?;
        Ok(found)
    }
}
