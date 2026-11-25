use std::collections::HashMap;

use async_trait::async_trait;
use nullradix_app::favorites::{FavoriteRepository, FavoriteWrite, StoredFavorite};
use nullradix_app::sessions::{StoredSession, session_not_found};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    Favorite, FavoriteRecord, Id, Repository, Resumption, SessionRef, SessionState, TicketKey,
    Timestamp, Version,
};
use sqlx::{Sqlite, SqliteConnection, Transaction};
use uuid::Uuid;

use crate::error::{database, missed_guard};
use crate::sessions::one;
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid favorite {what}"))
}

struct Row {
    session_id: String,
    repository_id: Option<Uuid>,
    note: String,
    cwd: String,
    title: Option<String>,
    ticket_keys: String,
    last_state: String,
    last_event_at: i64,
    marked_at: i64,
    version: i64,
}

struct ResumeRow {
    favorite_session_id: String,
    resumed_session_id: String,
    resumed_at: i64,
}

fn session_ref(text: &str) -> Result<SessionRef, StoreError> {
    SessionRef::parse(text).map_err(|_| corrupt("session"))
}

impl Row {
    fn into_favorite(self, resumed_as: Vec<Resumption>) -> Result<Favorite, StoreError> {
        let keys: Vec<String> =
            serde_json::from_str(&self.ticket_keys).map_err(|_| corrupt("ticket keys"))?;
        let tickets = keys
            .iter()
            .map(|key| TicketKey::parse(key).map_err(|_| corrupt("ticket key")))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Favorite::restore(FavoriteRecord {
            session: session_ref(&self.session_id)?,
            repository: self.repository_id.map(Id::from_uuid),
            note: self.note,
            cwd: self.cwd,
            title: self.title,
            tickets,
            last_state: self
                .last_state
                .parse::<SessionState>()
                .map_err(|_| corrupt("state"))?,
            last_event_at: Timestamp::from_millis(self.last_event_at),
            marked_at: Timestamp::from_millis(self.marked_at),
            resumed_as,
            version: Version::from_stored(self.version),
        }))
    }
}

/// Favorites newest marked first, with their resumptions.
async fn load(
    connection: &mut SqliteConnection,
    repository: Option<Uuid>,
    only: Option<&str>,
) -> Result<Vec<Favorite>, StoreError> {
    let rows = sqlx::query_as!(
        Row,
        r#"SELECT session_id AS "session_id!", repository_id AS "repository_id?: Uuid", note,
                  cwd, title, ticket_keys, last_state, last_event_at, marked_at, version
           FROM favorites
           WHERE (?1 IS NULL OR repository_id = ?1) AND (?2 IS NULL OR session_id = ?2)
           ORDER BY marked_at DESC, session_id"#,
        repository,
        only,
    )
    .fetch_all(&mut *connection)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let resume_rows = sqlx::query_as!(
        ResumeRow,
        r#"SELECT r.favorite_session_id, r.resumed_session_id, r.resumed_at
           FROM favorite_resumes r JOIN favorites f ON f.session_id = r.favorite_session_id
           WHERE (?1 IS NULL OR f.repository_id = ?1) AND (?2 IS NULL OR f.session_id = ?2)
           ORDER BY r.favorite_session_id, r.position"#,
        repository,
        only,
    )
    .fetch_all(&mut *connection)
    .await?;
    let mut resumed: HashMap<String, Vec<Resumption>> = HashMap::new();
    for row in resume_rows {
        resumed
            .entry(row.favorite_session_id)
            .or_default()
            .push(Resumption {
                session: session_ref(&row.resumed_session_id)?,
                at: Timestamp::from_millis(row.resumed_at),
            });
    }
    rows.into_iter()
        .map(|row| {
            let resumptions = resumed.remove(&row.session_id).unwrap_or_default();
            row.into_favorite(resumptions)
        })
        .collect()
}

/// Each favorite with its session while that exists.
async fn with_sessions(
    connection: &mut SqliteConnection,
    favorites: Vec<Favorite>,
) -> Result<Vec<StoredFavorite>, StoreError> {
    let mut stored = Vec::with_capacity(favorites.len());
    for favorite in favorites {
        let live = one(connection, &favorite.record().session).await?;
        stored.push(StoredFavorite { favorite, live });
    }
    Ok(stored)
}

fn ticket_keys(favorite: &Favorite) -> Result<String, StoreError> {
    let keys: Vec<String> = favorite
        .record()
        .tickets
        .iter()
        .map(ToString::to_string)
        .collect();
    serde_json::to_string(&keys).map_err(|_| corrupt("ticket keys"))
}

async fn save_resumes(
    connection: &mut SqliteConnection,
    favorite: &Favorite,
) -> Result<(), StoreError> {
    let id = favorite.record().session.as_str();
    sqlx::query!(
        "DELETE FROM favorite_resumes WHERE favorite_session_id = ?",
        id
    )
    .execute(&mut *connection)
    .await?;
    for (position, resumption) in favorite.record().resumed_as.iter().enumerate() {
        let position = i64::try_from(position).map_err(|_| corrupt("resume position"))?;
        let resumed = resumption.session.as_str();
        let at = resumption.at.as_millis();
        sqlx::query!(
            "INSERT INTO favorite_resumes (favorite_session_id, resumed_session_id, position,
                                           resumed_at)
             VALUES (?, ?, ?, ?)",
            id,
            resumed,
            position,
            at,
        )
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

/// Saves the favorite; fails with a version conflict when it is not stored at `previous`.
pub(crate) async fn save(
    connection: &mut SqliteConnection,
    favorite: &Favorite,
    previous: Version,
) -> Result<Change, AppError> {
    let record = favorite.record();
    let id = record.session.as_str();
    let repository = record.repository.map(|id| id.as_uuid());
    let keys = ticket_keys(favorite)?;
    let state = record.last_state.as_str();
    let last_event_at = record.last_event_at.as_millis();
    let version = record.version.value();
    let previous = previous.value();
    let updated = sqlx::query_scalar!(
        r#"UPDATE favorites
               SET repository_id = ?, note = ?, cwd = ?, title = ?, ticket_keys = ?,
                   last_state = ?, last_event_at = ?, version = ?
               WHERE session_id = ? AND version = ?
               RETURNING feed_id AS "feed_id: Uuid""#,
        repository,
        record.note,
        record.cwd,
        record.title,
        keys,
        state,
        last_event_at,
        version,
        id,
        previous,
    )
    .fetch_optional(&mut *connection)
    .await
    .map_err(database)?;
    let Some(feed_id) = updated else {
        let exists = sqlx::query_scalar!("SELECT version FROM favorites WHERE session_id = ?", id)
            .fetch_optional(&mut *connection)
            .await;
        return Err(missed_guard(
            exists,
            AppError::version_conflict("favorite", id),
            session_not_found(id),
        ));
    };
    save_resumes(&mut *connection, favorite).await?;
    Ok(change_feed::record(
        &mut *connection,
        Entity::Favorite,
        feed_id,
        ChangeOp::Update,
        record.last_event_at,
    )
    .await?)
}

pub(crate) async fn find(
    connection: &mut SqliteConnection,
    id: &SessionRef,
) -> Result<Option<Favorite>, StoreError> {
    Ok(load(connection, None, Some(id.as_str())).await?.pop())
}

pub(crate) async fn folders(
    connection: &mut SqliteConnection,
) -> Result<Vec<(SessionRef, String)>, StoreError> {
    let rows = sqlx::query!(r#"SELECT session_id AS "session_id!", cwd FROM favorites"#)
        .fetch_all(&mut *connection)
        .await?;
    rows.into_iter()
        .map(|row| Ok((session_ref(&row.session_id)?, row.cwd)))
        .collect()
}

pub(crate) struct Write(Transaction<'static, Sqlite>);

#[async_trait]
impl FavoriteWrite for Write {
    async fn session(&mut self, id: &SessionRef) -> Result<Option<StoredSession>, AppError> {
        Ok(one(&mut self.0, id).await?)
    }

    async fn favorite(&mut self, id: &SessionRef) -> Result<Option<Favorite>, AppError> {
        Ok(find(&mut self.0, id).await?)
    }

    async fn folders(&mut self) -> Result<Vec<(SessionRef, String)>, AppError> {
        Ok(folders(&mut self.0).await?)
    }

    async fn insert(&mut self, favorite: &Favorite, feed_id: Uuid) -> Result<Change, AppError> {
        let record = favorite.record();
        let id = record.session.as_str();
        let repository = record.repository.map(|id| id.as_uuid());
        let keys = ticket_keys(favorite)?;
        let state = record.last_state.as_str();
        let last_event_at = record.last_event_at.as_millis();
        let marked_at = record.marked_at.as_millis();
        let version = record.version.value();
        sqlx::query!(
            "INSERT INTO favorites (session_id, feed_id, repository_id, note, cwd, title,
                                    ticket_keys, last_state, last_event_at, marked_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            feed_id,
            repository,
            record.note,
            record.cwd,
            record.title,
            keys,
            state,
            last_event_at,
            marked_at,
            version,
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        save_resumes(&mut self.0, favorite).await?;
        Ok(change_feed::record(
            &mut self.0,
            Entity::Favorite,
            feed_id,
            ChangeOp::Insert,
            record.marked_at,
        )
        .await?)
    }

    async fn update(&mut self, favorite: &Favorite, previous: Version) -> Result<Change, AppError> {
        save(&mut self.0, favorite, previous).await
    }

    async fn delete(&mut self, id: &SessionRef, at: Timestamp) -> Result<Option<Change>, AppError> {
        let id = id.as_str();
        let gone = sqlx::query_scalar!(
            r#"DELETE FROM favorites WHERE session_id = ? RETURNING feed_id AS "feed_id: Uuid""#,
            id
        )
        .fetch_optional(&mut *self.0)
        .await
        .map_err(database)?;
        let Some(feed_id) = gone else {
            return Ok(None);
        };
        Ok(Some(
            change_feed::record(&mut self.0, Entity::Favorite, feed_id, ChangeOp::Delete, at)
                .await?,
        ))
    }

    async fn commit(self: Box<Self>) -> Result<(), AppError> {
        self.0.commit().await.map_err(database)
    }
}

#[async_trait]
impl FavoriteRepository for Store {
    async fn begin(&self) -> Result<Box<dyn FavoriteWrite>, AppError> {
        Ok(Box::new(Write(self.begin().await?)))
    }

    async fn list(
        &self,
        repository: Option<Id<Repository>>,
    ) -> Result<Vec<StoredFavorite>, AppError> {
        // one read transaction keeps favorites and their sessions on the same snapshot
        let mut read = self.reader.begin().await.map_err(database)?;
        let favorites = load(&mut read, repository.map(|id| id.as_uuid()), None).await?;
        let stored = with_sessions(&mut read, favorites).await?;
        read.commit().await.map_err(database)?;
        Ok(stored)
    }

    async fn get(&self, id: &SessionRef) -> Result<Option<StoredFavorite>, AppError> {
        let mut read = self.reader.begin().await.map_err(database)?;
        let favorites = load(&mut read, None, Some(id.as_str())).await?;
        let stored = with_sessions(&mut read, favorites).await?;
        read.commit().await.map_err(database)?;
        Ok(stored.into_iter().next())
    }
}
