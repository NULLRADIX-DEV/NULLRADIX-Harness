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
    /// Comments of the ticket, oldest first; `None` when the ticket does not exist.
    async fn list(&self, ticket: &TicketKey) -> Result<Option<Vec<CommentLine>>, AppError>;
    /// Comments of these tickets no agent session got yet, oldest first.
    async fn undelivered(&self, tickets: &[Id<Ticket>]) -> Result<Vec<CommentLine>, AppError>;
    /// Marks the comments as delivered; the tickets keep their version.
    async fn mark_delivered(&self, ids: &[Id<Comment>], at: Timestamp) -> Result<(), AppError>;
}

/// Who writes a comment: a person in the browser or an agent with a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Author<'a> {
    pub by_person: bool,
    /// Name to write under, such as `claude`; the acting person when left out or blank.
    pub name: Option<&'a str>,
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
            !author.by_person,
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
        self.comments.mark_delivered(ids, self.clock.now()).await
    }

    /// The named author, as a person or a session by who writes; else the acting person.
    async fn author(&self, author: Author<'_>, person: &Actor) -> Result<Actor, AppError> {
        let Some(name) = author.name.filter(|name| !name.trim().is_empty()) else {
            return Ok(person.clone());
        };
        let name = ActorName::parse(name).map_err(|error| author_invalid(&error))?;
        let kind = if author.by_person {
            ActorKind::Human
        } else {
            ActorKind::Session
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
