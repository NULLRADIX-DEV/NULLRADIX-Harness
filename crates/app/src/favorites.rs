//! Favorite sessions: marked with a note, kept past their session, linked to their resumptions.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Favorite, Id, Repository, RunnerOs, Session, SessionRef, Timestamp, Version, resumed_favorite,
};
use uuid::Uuid;

use crate::repositories::RunnerService;
use crate::sessions::{SessionLine, StoredSession, session_id_invalid, session_not_found};
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

/// A favorite with its session while that still exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFavorite {
    pub favorite: Favorite,
    pub live: Option<StoredSession>,
}

/// A favorite as lists show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FavoriteLine {
    pub favorite: Favorite,
    pub live: Option<SessionLine>,
    /// Whether the folder still exists on the runner, so the resume command can work.
    pub cwd_exists: bool,
}

/// One write; dropping it without commit rolls back.
#[async_trait]
pub trait FavoriteWrite: Send {
    async fn session(&mut self, id: &SessionRef) -> Result<Option<StoredSession>, AppError>;
    async fn favorite(&mut self, id: &SessionRef) -> Result<Option<Favorite>, AppError>;
    /// Session and folder of every favorite.
    async fn folders(&mut self) -> Result<Vec<(SessionRef, String)>, AppError>;
    async fn insert(&mut self, favorite: &Favorite, feed_id: Uuid) -> Result<Change, AppError>;
    /// Fails with a version conflict when the stored favorite is not at `previous`.
    async fn update(&mut self, favorite: &Favorite, previous: Version) -> Result<Change, AppError>;
    /// Removes the favorite; `None` when there was none.
    async fn delete(&mut self, id: &SessionRef, at: Timestamp) -> Result<Option<Change>, AppError>;
    async fn commit(self: Box<Self>) -> Result<(), AppError>;
}

#[async_trait]
pub trait FavoriteRepository: Send + Sync {
    async fn begin(&self) -> Result<Box<dyn FavoriteWrite>, AppError>;
    /// Latest marked first; all repositories when `repository` is `None`.
    async fn list(
        &self,
        repository: Option<Id<Repository>>,
    ) -> Result<Vec<StoredFavorite>, AppError>;
    async fn get(&self, id: &SessionRef) -> Result<Option<StoredFavorite>, AppError>;
}

pub struct FavoriteService {
    repository: Arc<dyn FavoriteRepository>,
    runners: Arc<RunnerService>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for FavoriteService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FavoriteService").finish_non_exhaustive()
    }
}

fn keys(stored: &StoredSession) -> Vec<nullradix_domain::TicketKey> {
    stored
        .tickets
        .iter()
        .map(|ticket| ticket.key.clone())
        .collect()
}

impl FavoriteService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn FavoriteRepository>,
        runners: Arc<RunnerService>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            repository,
            runners,
            clock,
            ids,
            signal,
        }
    }

    /// Marks the session, or takes its snapshot again; a gone session keeps only its note.
    pub async fn mark(&self, id: &str, note: Option<&str>) -> Result<FavoriteLine, AppError> {
        let session = SessionRef::parse(id).map_err(|_| session_id_invalid())?;
        let now = self.clock.now();
        let mut write = self.repository.begin().await?;
        let live = write.session(&session).await?;
        let change = match (write.favorite(&session).await?, &live) {
            (None, Some(stored)) => {
                let favorite = Favorite::mark(&stored.session, keys(stored), note, now);
                write.insert(&favorite, self.ids.next_uuid()).await?
            }
            (Some(known), Some(stored)) => {
                let next = known.remarked(&stored.session, keys(stored), note);
                write.update(&next, known.record().version).await?
            }
            // as legacy: a favorite whose session is gone only takes the note
            (Some(known), None) => {
                let next = known.with_note(note.unwrap_or_default());
                write.update(&next, known.record().version).await?
            }
            (None, None) => return Err(session_not_found(session.as_str())),
        };
        write.commit().await?;
        self.signal.notify(&change);
        let stored = self
            .repository
            .get(&session)
            .await?
            .ok_or_else(|| session_not_found(session.as_str()))?;
        Ok(self.lines(vec![stored]).await?.remove(0))
    }

    /// Removes the mark; a session that was not marked is no error, as in legacy.
    pub async fn unmark(&self, id: &str) -> Result<(), AppError> {
        let session = SessionRef::parse(id).map_err(|_| session_id_invalid())?;
        let mut write = self.repository.begin().await?;
        let change = write.delete(&session, self.clock.now()).await?;
        write.commit().await?;
        if let Some(change) = &change {
            self.signal.notify(change);
        }
        Ok(())
    }

    pub async fn list(
        &self,
        repository: Option<Id<Repository>>,
    ) -> Result<Vec<FavoriteLine>, AppError> {
        let stored = self.repository.list(repository).await?;
        self.lines(stored).await
    }

    /// After a hook: the snapshot follows the session, and a resumed one is linked.
    pub async fn follow(&self, session: &Session, resumed: bool) -> Result<(), AppError> {
        let id = session.id();
        let mut write = self.repository.begin().await?;
        let mut changes = Vec::new();
        if let Some(known) = write.favorite(id).await? {
            let tickets = match write.session(id).await? {
                Some(stored) => keys(&stored),
                None => known.record().tickets.clone(),
            };
            let next = known.refreshed(session, tickets);
            changes.push(write.update(&next, known.record().version).await?);
        }
        if resumed {
            let fold = self.runners.local().os() == RunnerOs::Windows;
            let folders = write.folders().await?;
            if let Some(owner) = resumed_favorite(&folders, &session.record().cwd, fold)
                && let Some(known) = write.favorite(owner).await?
                && let Some(next) = known.resumed(id, session.record().last_event_at)
            {
                changes.push(write.update(&next, known.record().version).await?);
            }
        }
        write.commit().await?;
        for change in &changes {
            self.signal.notify(change);
        }
        Ok(())
    }

    async fn lines(&self, stored: Vec<StoredFavorite>) -> Result<Vec<FavoriteLine>, AppError> {
        let now = self.clock.now();
        let folders: Vec<String> = stored
            .iter()
            .map(|item| item.favorite.record().cwd.clone())
            .collect();
        let local = self.runners.local().id();
        // the folder check only guides the person, so a failing runner must not hide the list
        let exists = match self.runners.folders_exist(local, &folders).await {
            Ok(exists) => exists,
            Err(error) => {
                tracing::warn!(%error, "cannot check the folders of favorites");
                vec![true; folders.len()]
            }
        };
        Ok(stored
            .into_iter()
            .zip(exists)
            .map(|(item, cwd_exists)| FavoriteLine {
                favorite: item.favorite,
                live: item.live.map(|live| SessionLine {
                    view: live.session.view(now),
                    session: live.session,
                    tickets: live.tickets,
                }),
                cwd_exists,
            })
            .collect())
    }
}
