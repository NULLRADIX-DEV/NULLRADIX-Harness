//! Comments on tickets: appended in the ticket's write, read in time order.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Actor, ActorKind, ActorName, BoardError, Comment, CommentBody, CommentError, Id, Ticket,
    TicketKey, Timestamp,
};

use crate::boards::ActorRepository;
use crate::tickets::{self, TicketRepository, load};
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

/// A comment with the ticket and author it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentLine {
    pub comment: Comment,
    pub ticket_key: TicketKey,
    pub author_name: String,
    pub author_kind: ActorKind,
}

/// Comment changes inside a ticket write, so they commit together with the ticket.
#[async_trait]
pub trait CommentWrite: Send {
    async fn insert_comment(&mut self, comment: &Comment) -> Result<Change, AppError>;
}

#[async_trait]
pub trait CommentRepository: Send + Sync {
    async fn ticket_exists(&self, ticket: &TicketKey) -> Result<bool, AppError>;
    /// Comments of the ticket, oldest first; `None` when the ticket does not exist.
    async fn list(&self, ticket: &TicketKey) -> Result<Option<Vec<CommentLine>>, AppError>;
    /// Comments of these tickets no agent session got yet, oldest first.
    async fn undelivered(&self, tickets: &[Id<Ticket>]) -> Result<Vec<CommentLine>, AppError>;
    /// Marks comments delivered, one change each that was not yet; returns the last change.
    async fn mark_delivered(
        &self,
        ids: &[Id<Comment>],
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
}

/// Who writes a comment: the person in the browser, or a caller with a token.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Author<'a> {
    /// A browser session always writes as the person, undelivered.
    pub by_person: bool,
    /// Name to write under, such as `claude`; the acting person when left out or blank.
    pub name: Option<&'a str>,
    /// The agent session the caller runs in, as legacy read `NULLRADIX_SESSION`.
    pub session: Option<&'a str>,
}

impl Author<'_> {
    /// Written inside an agent session, so it counts as delivered to agents at once.
    fn in_session(self) -> bool {
        !self.by_person
            && self
                .session
                .is_some_and(|session| !session.trim().is_empty())
    }
}

#[must_use]
pub fn invalid(error: &CommentError) -> AppError {
    let CommentError::Body(text) = error;
    AppError::text(*text, "comment-empty", error.to_string())
}

fn author_invalid(error: &BoardError) -> AppError {
    let text = match error {
        BoardError::ActorName(text) | BoardError::Name(text) => *text,
    };
    AppError::text(text, "author-empty", error.to_string())
}

pub struct CommentService {
    tickets: Arc<dyn TicketRepository>,
    comments: Arc<dyn CommentRepository>,
    actors: Arc<dyn ActorRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for CommentService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommentService").finish_non_exhaustive()
    }
}

impl CommentService {
    #[must_use]
    pub fn new(
        tickets: Arc<dyn TicketRepository>,
        comments: Arc<dyn CommentRepository>,
        actors: Arc<dyn ActorRepository>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            tickets,
            comments,
            actors,
            clock,
            ids,
            signal,
        }
    }

    /// Appends a comment; the ticket gets a new version but no history line, as in legacy.
    pub async fn add(
        &self,
        key: &str,
        body: &str,
        author: Author<'_>,
        person: &Actor,
    ) -> Result<CommentLine, AppError> {
        let key = TicketKey::parse(key).map_err(|error| tickets::invalid(&error))?;
        let body = CommentBody::parse(body).map_err(|error| invalid(&error))?;
        if !self.comments.ticket_exists(&key).await? {
            return Err(tickets::not_found(&key));
        }
        // the actor commits on its own, before the ticket write takes the connection
        let actor = self.author(author, person).await?;
        let mut write = self.tickets.begin().await?;
        let ticket = load(write.as_mut(), &key, None).await?;
        let now = self.clock.now();
        let comment = Comment::new(
            self.ids.next(),
            ticket.id(),
            actor.id(),
            body,
            author.in_session(),
            now,
        );
        let revised = ticket.revised(ticket.record().clone(), now);
        write.update(&revised, ticket.record().version, &[]).await?;
        let change = write.insert_comment(&comment).await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(CommentLine {
            comment,
            ticket_key: key,
            author_name: actor.name().as_str().to_owned(),
            author_kind: actor.kind(),
        })
    }

    /// Comments of the ticket, oldest first.
    pub async fn list(&self, key: &str) -> Result<Vec<CommentLine>, AppError> {
        let key = TicketKey::parse(key).map_err(|error| tickets::invalid(&error))?;
        self.comments
            .list(&key)
            .await?
            .ok_or_else(|| tickets::not_found(&key))
    }

    /// Comments of these tickets that no agent session got yet, oldest first.
    pub async fn undelivered(&self, tickets: &[Id<Ticket>]) -> Result<Vec<CommentLine>, AppError> {
        if tickets.is_empty() {
            return Ok(Vec::new());
        }
        self.comments.undelivered(tickets).await
    }

    /// Records that an agent session got these comments.
    pub async fn mark_delivered(&self, ids: &[Id<Comment>]) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }
        if let Some(change) = self.comments.mark_delivered(ids, self.clock.now()).await? {
            self.signal.notify(&change);
        }
        Ok(())
    }

    /// The named author, a session inside one and a human otherwise; else the acting person.
    async fn author(&self, author: Author<'_>, person: &Actor) -> Result<Actor, AppError> {
        let named = author.name.filter(|name| !name.trim().is_empty());
        let (Some(name), false) = (named, author.by_person) else {
            return Ok(person.clone());
        };
        let name = ActorName::parse(name).map_err(|error| author_invalid(&error))?;
        let kind = if author.in_session() {
            ActorKind::Session
        } else {
            ActorKind::Human
        };
        let candidate = Actor::new(self.ids.next(), kind, name, self.clock.now());
        self.actors.ensure(&candidate).await
    }
}

#[cfg(test)]
mod tests {
    use nullradix_domain::TextError;

    use super::*;

    #[test]
    fn maps_comment_errors_to_codes() {
        assert_eq!(
            invalid(&CommentError::Body(TextError::Empty)).code(),
            "comment-empty"
        );
        assert_eq!(
            author_invalid(&BoardError::ActorName(TextError::TooLong(100))).code(),
            "value-too-long"
        );
    }
}
