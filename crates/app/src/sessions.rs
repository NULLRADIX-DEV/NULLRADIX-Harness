//! Claude Code sessions, built from hook events in the write that records the event.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use nullradix_domain::{
    DisplayState, HookEvent, Id, Repository, Runner, RunnerOs, Session, SessionRef, SessionView,
    Ticket, TicketKey, TicketStatus, Timestamp, Version, path_within, session_prune_before,
    ticket_names,
};
use parking_lot::Mutex;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::repositories::RunnerService;
use crate::tickets;
use crate::{AppError, Change, ChangeSignal, Clock, ErrorKind, IdGenerator};

/// A cwd outside every known checkout is looked up again after this long.
const UNKNOWN_FOR: Duration = Duration::from_mins(1);

/// A ticket a session works on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTicket {
    pub key: TicketKey,
    pub title: String,
    pub status: TicketStatus,
}

/// A session with its tickets and the figures a list computes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLine {
    pub session: Session,
    pub view: SessionView,
    pub tickets: Vec<SessionTicket>,
}

/// A stored session with its tickets, before the view is computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSession {
    pub session: Session,
    pub tickets: Vec<SessionTicket>,
}

/// Which sessions a list shows; all when every field is empty.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionScope {
    pub repository: Option<Id<Repository>>,
    pub ticket: Option<Id<Ticket>>,
}

/// One write; dropping it without commit rolls back.
#[async_trait]
pub trait SessionWrite: Send {
    /// Keeps the hook event unless one with its id exists; true when it was new.
    async fn record_hook(
        &mut self,
        event: &HookEvent,
        received_at: Timestamp,
    ) -> Result<bool, AppError>;
    async fn session(&mut self, id: &SessionRef) -> Result<Option<Session>, AppError>;
    async fn insert(&mut self, session: &Session, feed_id: Uuid) -> Result<Change, AppError>;
    /// Fails with a version conflict when the stored session is not at `previous`.
    async fn update(&mut self, session: &Session, previous: Version) -> Result<Change, AppError>;
    /// A change for the session without touching its row, after a new ticket link.
    async fn touch(&mut self, id: &SessionRef, at: Timestamp) -> Result<Change, AppError>;
    /// Takes the next session number of the repository; `None` when it does not exist.
    async fn next_number(&mut self, repository: Id<Repository>) -> Result<Option<u32>, AppError>;
    async fn ticket(&mut self, key: &TicketKey) -> Result<Option<Id<Ticket>>, AppError>;
    /// Links the ticket; true when it was not linked yet.
    async fn link(
        &mut self,
        ticket: Id<Ticket>,
        session: &SessionRef,
        at: Timestamp,
    ) -> Result<bool, AppError>;
    async fn linked(&mut self, session: &SessionRef) -> Result<bool, AppError>;
    /// Folders of live worktrees on `runner` with the in-progress tickets working in them.
    async fn worktree_tickets(
        &mut self,
        runner: Id<Runner>,
    ) -> Result<Vec<(String, Id<Ticket>)>, AppError>;
    /// Sessions in `cwd` whose repository is not known yet.
    async fn unlocated(&mut self, cwd: &str) -> Result<Vec<Session>, AppError>;
    async fn commit(self: Box<Self>) -> Result<(), AppError>;
}

#[async_trait]
pub trait SessionRepository: Send + Sync {
    async fn begin(&self) -> Result<Box<dyn SessionWrite>, AppError>;
    async fn ticket_id(&self, key: &TicketKey) -> Result<Option<Id<Ticket>>, AppError>;
    /// Latest activity first; the latest report first on equal times.
    async fn list(&self, scope: SessionScope) -> Result<Vec<StoredSession>, AppError>;
    async fn get(&self, id: &SessionRef) -> Result<Option<StoredSession>, AppError>;
    /// Deletes sessions that ended or went silent before `before`; count and last change.
    async fn prune_before(
        &self,
        before: Timestamp,
        now: Timestamp,
    ) -> Result<(u64, Option<Change>), AppError>;
}

#[must_use]
pub fn session_not_found(id: &str) -> AppError {
    AppError::not_found("session-not-found", format!("session {id} does not exist"))
}

#[must_use]
pub fn state_invalid(state: &str) -> AppError {
    AppError::invalid(
        "state-invalid",
        format!("{state} is none of idle, running, waiting, ended and stale"),
    )
}

#[must_use]
pub fn session_id_invalid() -> AppError {
    AppError::invalid(
        "session-id-invalid",
        "a session id has 1 to 200 characters and no control characters",
    )
}

/// Filters of a session list as the API receives them.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionQuery<'a> {
    pub repository: Option<Id<Repository>>,
    /// A display state such as `running` or `stale`.
    pub state: Option<&'a str>,
    pub ticket: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
enum Located {
    Pending,
    Known(Option<Id<Repository>>, Instant),
}

pub struct SessionService {
    repository: Arc<dyn SessionRepository>,
    runners: Arc<RunnerService>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
    located: Mutex<HashMap<String, Located>>,
    lookups: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for SessionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionService").finish_non_exhaustive()
    }
}

impl SessionService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn SessionRepository>,
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
            located: Mutex::new(HashMap::new()),
            lookups: Mutex::new(Vec::new()),
        }
    }

    /// Records the hook event and applies it to its session in one write; true when it was new.
    pub async fn receive(self: &Arc<Self>, event: &HookEvent) -> Result<bool, AppError> {
        let known = self.known(event.cwd());
        let now = self.clock.now();
        let mut write = self.repository.begin().await?;
        if !write.record_hook(event, now).await? {
            return Ok(false);
        }
        let id = event.session();
        let current = write.session(id).await?;
        let runner = self.runners.local().id();
        let base = current
            .clone()
            .unwrap_or_else(|| Session::begin(event, Some(runner)));
        let mut next = base.apply(event);
        let shown = next.as_ref().unwrap_or(&base);
        if shown.record().repository.is_none()
            && let Some(repository) = known
            && let Some(number) = write.next_number(repository).await?
        {
            next = Some(shown.located(repository, number));
        }
        let mut change = match (&next, &current) {
            (Some(session), None) => Some(write.insert(session, self.ids.next_uuid()).await?),
            (Some(session), Some(stored)) => {
                Some(write.update(session, stored.record().version).await?)
            }
            (None, _) => None,
        };
        let session = next.unwrap_or(base);
        if self.link_tickets(write.as_mut(), event, &session).await? && change.is_none() {
            change = Some(write.touch(id, now).await?);
        }
        write.commit().await?;
        if let Some(change) = &change {
            self.signal.notify(change);
        }
        // the event is stored; a failed lookup must not fail its hook
        if session.record().repository.is_none()
            && let Err(error) = self.locate_later(event.cwd()).await
        {
            tracing::warn!(%error, cwd = event.cwd(), "cannot place the session in its repository");
        }
        Ok(true)
    }

    /// Links the tickets the hook names, else those whose worktree holds the cwd, as legacy.
    async fn link_tickets(
        &self,
        write: &mut dyn SessionWrite,
        event: &HookEvent,
        session: &Session,
    ) -> Result<bool, AppError> {
        let at = event.occurred_at();
        let id = event.session();
        let mut named = Vec::new();
        for name in ticket_names(event.details().ticket.as_deref()) {
            if let Ok(key) = TicketKey::parse(&name)
                && let Some(ticket) = write.ticket(&key).await?
            {
                named.push(ticket);
            }
        }
        let mut linked = false;
        if named.is_empty() {
            let Some(runner) = session.record().runner else {
                return Ok(false);
            };
            if write.linked(id).await? {
                return Ok(false);
            }
            let fold = self.runners.local().os() == RunnerOs::Windows;
            for (folder, ticket) in write.worktree_tickets(runner).await? {
                if path_within(&folder, event.cwd(), fold) {
                    linked |= write.link(ticket, id, at).await?;
                }
            }
            return Ok(linked);
        }
        for ticket in named {
            linked |= write.link(ticket, id, at).await?;
        }
        Ok(linked)
    }

    /// The repository of `cwd` once a lookup found one.
    fn known(&self, cwd: &str) -> Option<Id<Repository>> {
        match self.located.lock().get(cwd) {
            Some(Located::Known(Some(repository), _)) => Some(*repository),
            _ => None,
        }
    }

    /// Looks up the repository of `cwd` in the background so the hook is not kept waiting.
    async fn locate_later(self: &Arc<Self>, cwd: &str) -> Result<(), AppError> {
        let start = {
            let mut located = self.located.lock();
            match located.get(cwd) {
                Some(Located::Pending) => false,
                Some(Located::Known(Some(_), _)) => true,
                Some(Located::Known(None, at)) if at.elapsed() < UNKNOWN_FOR => false,
                _ => {
                    located.insert(cwd.to_owned(), Located::Pending);
                    true
                }
            }
        };
        if !start {
            return Ok(());
        }
        // found meanwhile: the event came after the lookup placed the others
        if let Some(repository) = self.known(cwd) {
            return self.place(cwd, repository).await;
        }
        let service = Arc::clone(self);
        let cwd = cwd.to_owned();
        let lookup = tokio::spawn(async move { service.locate(cwd).await });
        let mut lookups = self.lookups.lock();
        lookups.retain(|lookup| !lookup.is_finished());
        lookups.push(lookup);
        Ok(())
    }

    async fn locate(&self, cwd: String) {
        let runner = self.runners.local().id();
        let repository = match self.runners.recorded(runner, &cwd).await {
            Ok((_, repository)) => repository.map(|repository| repository.id()),
            Err(error) if error.kind() == ErrorKind::Invalid => None,
            Err(error) => {
                tracing::warn!(%error, cwd = %cwd, "cannot locate the session folder");
                self.located.lock().remove(&cwd);
                return;
            }
        };
        // placed first, so a hook that finds the repository known finds no one left to place
        if let Some(repository) = repository
            && let Err(error) = self.place(&cwd, repository).await
        {
            tracing::warn!(%error, cwd = %cwd, "cannot place sessions in their repository");
            self.located.lock().remove(&cwd);
            return;
        }
        self.located
            .lock()
            .insert(cwd, Located::Known(repository, Instant::now()));
    }

    /// Gives sessions in `cwd` without a repository this one and their numbers.
    async fn place(&self, cwd: &str, repository: Id<Repository>) -> Result<(), AppError> {
        let mut write = self.repository.begin().await?;
        let mut last = None;
        for session in write.unlocated(cwd).await? {
            let Some(number) = write.next_number(repository).await? else {
                break;
            };
            let located = session.located(repository, number);
            last = Some(write.update(&located, session.record().version).await?);
        }
        write.commit().await?;
        if let Some(change) = &last {
            self.signal.notify(change);
        }
        Ok(())
    }

    /// Waits for repository lookups started so far; for tests and shutdown.
    pub async fn settled(&self) {
        let lookups: Vec<JoinHandle<()>> = std::mem::take(&mut *self.lookups.lock());
        for lookup in lookups {
            if let Err(error) = lookup.await {
                tracing::warn!(%error, "repository lookup failed");
            }
        }
    }

    pub async fn list(&self, query: SessionQuery<'_>) -> Result<Vec<SessionLine>, AppError> {
        let state = query
            .state
            .map(|state| {
                state
                    .parse::<DisplayState>()
                    .map_err(|_| state_invalid(state))
            })
            .transpose()?;
        let ticket = match query.ticket {
            Some(key) => Some(self.ticket(key).await?),
            None => None,
        };
        let scope = SessionScope {
            repository: query.repository,
            ticket,
        };
        let now = self.clock.now();
        Ok(self
            .repository
            .list(scope)
            .await?
            .into_iter()
            .filter(|stored| !stored.session.expired(now))
            .map(|stored| line(stored, now))
            .filter(|line| state.is_none_or(|state| line.view.display_state == state))
            .collect())
    }

    /// Sessions linked to the ticket, latest activity first.
    pub async fn for_ticket(&self, key: &str) -> Result<Vec<SessionLine>, AppError> {
        self.list(SessionQuery {
            ticket: Some(key),
            ..SessionQuery::default()
        })
        .await
    }

    pub async fn get(&self, id: &str) -> Result<SessionLine, AppError> {
        let session = SessionRef::parse(id).map_err(|_| session_id_invalid())?;
        let stored = self
            .repository
            .get(&session)
            .await?
            .ok_or_else(|| session_not_found(session.as_str()))?;
        Ok(line(stored, self.clock.now()))
    }

    /// Forgets sessions that ended or went silent a week ago, as legacy did.
    pub async fn prune(&self) -> Result<u64, AppError> {
        let now = self.clock.now();
        let before = session_prune_before(now);
        let (count, change) = self.repository.prune_before(before, now).await?;
        if let Some(change) = &change {
            self.signal.notify(change);
        }
        Ok(count)
    }

    async fn ticket(&self, key: &str) -> Result<Id<Ticket>, AppError> {
        let key = TicketKey::parse(key).map_err(|error| tickets::invalid(&error))?;
        self.repository
            .ticket_id(&key)
            .await?
            .ok_or_else(|| tickets::not_found(&key))
    }
}

fn line(stored: StoredSession, now: Timestamp) -> SessionLine {
    SessionLine {
        view: stored.session.view(now),
        session: stored.session,
        tickets: stored.tickets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_have_stable_codes() {
        assert_eq!(session_not_found("s").code(), "session-not-found");
        assert_eq!(session_not_found("s").kind(), ErrorKind::NotFound);
        assert_eq!(session_id_invalid().code(), "session-id-invalid");
        assert_eq!(session_id_invalid().kind(), ErrorKind::Invalid);
        assert_eq!(state_invalid("x").code(), "state-invalid");
    }
}
