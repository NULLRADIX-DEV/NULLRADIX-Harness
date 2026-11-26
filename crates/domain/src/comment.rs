//! Comments on tickets and questions: appended, never changed by people.

use strum::{EnumIter, EnumString, IntoStaticStr};

use crate::{Actor, Id, Question, Run, TextError, Ticket, Timestamp, Version, required_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CommentError {
    #[error("the comment {0}")]
    Body(TextError),
}

/// The text of a comment, trimmed and never empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentBody(String);

impl CommentBody {
    pub fn parse(text: &str) -> Result<Self, CommentError> {
        required_text(text, usize::MAX)
            .map(|body| Self(body.to_owned()))
            .map_err(CommentError::Body)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a dispatcher comment reports; legacy parsed it from German prefixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "snake_case")]
pub enum SystemMarker {
    Start,
    Abort,
    Pause,
    Question,
    Merged,
    MergeConflict,
    MergeWait,
    Merge,
    Setup,
    ResearchEmpty,
    PlanEmpty,
    Review,
    ReviewReturned,
}

impl SystemMarker {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// What a comment belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentOn {
    Ticket(Id<Ticket>),
    Question(Id<Question>),
}

/// Every stored field of a comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentRecord {
    pub id: Id<Comment>,
    pub subject: CommentOn,
    pub author: Id<Actor>,
    pub body: CommentBody,
    pub system_marker: Option<SystemMarker>,
    pub run: Option<Id<Run>>,
    /// When an agent session got the comment; legacy `ReadByAgentAt`.
    pub delivered_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment(CommentRecord);

impl Comment {
    /// A new comment; one an agent wrote counts as delivered to agents at once.
    #[must_use]
    pub const fn new(
        id: Id<Self>,
        subject: CommentOn,
        author: Id<Actor>,
        body: CommentBody,
        by_agent: bool,
        now: Timestamp,
    ) -> Self {
        Self(CommentRecord {
            id,
            subject,
            author,
            body,
            system_marker: None,
            run: None,
            delivered_at: if by_agent { Some(now) } else { None },
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        })
    }

    /// A comment of the dispatcher that marks an event, delivered to agents at once.
    #[must_use]
    pub const fn system(
        id: Id<Self>,
        subject: CommentOn,
        author: Id<Actor>,
        body: CommentBody,
        marker: SystemMarker,
        run: Option<Id<Run>>,
        now: Timestamp,
    ) -> Self {
        Self(CommentRecord {
            id,
            subject,
            author,
            body,
            system_marker: Some(marker),
            run,
            delivered_at: Some(now),
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        })
    }

    #[must_use]
    pub const fn restore(record: CommentRecord) -> Self {
        Self(record)
    }

    #[must_use]
    pub const fn record(&self) -> &CommentRecord {
        &self.0
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.0.id
    }

    #[must_use]
    pub fn body(&self) -> &str {
        self.0.body.as_str()
    }
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::*;

    fn id<T>(value: u128) -> Id<T> {
        Id::from_uuid(uuid::Uuid::from_u128(value))
    }

    #[test]
    fn the_body_is_trimmed_and_never_empty() {
        assert_eq!(
            CommentBody::parse("  Step 2 done \n").unwrap().as_str(),
            "Step 2 done"
        );
        assert_eq!(
            CommentBody::parse(" \t\n"),
            Err(CommentError::Body(TextError::Empty))
        );
        assert_eq!(CommentBody::parse("a\n\n  b").unwrap().as_str(), "a\n\n  b");
    }

    #[test]
    fn an_agent_comment_is_delivered_at_once_a_person_one_is_not() {
        let body = CommentBody::parse("x").unwrap();
        let now = Timestamp::from_millis(5);
        let (id, ticket, author) = (id(1), CommentOn::Ticket(id(2)), id(3));
        let by_agent = Comment::new(id, ticket, author, body.clone(), true, now);
        let by_person = Comment::new(id, ticket, author, body, false, now);
        assert_eq!(by_agent.record().delivered_at, Some(now));
        assert_eq!(by_person.record().delivered_at, None);
        assert_eq!(by_person.record().version, Version::INITIAL);
        assert_eq!(by_person.body(), "x");
    }

    #[test]
    fn markers_round_trip_their_stored_names() {
        for marker in SystemMarker::iter() {
            assert_eq!(marker.as_str().parse::<SystemMarker>(), Ok(marker));
        }
        assert_eq!(SystemMarker::MergeConflict.as_str(), "merge_conflict");
        assert!("Abbruch".parse::<SystemMarker>().is_err());
    }
}
