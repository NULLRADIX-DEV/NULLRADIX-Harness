//! Who writes under a name: one rule for comments, notes and the tickets made from notes.

use nullradix_domain::{Actor, ActorKind, ActorName, BoardError, Id};

use crate::boards::ActorRepository;
use crate::{AppError, Clock, IdGenerator};

/// Who writes: the person in the browser, or a caller with a token.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Author<'a> {
    /// A browser session always writes as the person, as legacy web routes did.
    pub by_person: bool,
    /// Name to write under, such as `claude`; the acting person when left out or blank.
    pub name: Option<&'a str>,
    /// The agent session the caller runs in, as legacy read `NULLRADIX_SESSION`.
    pub session: Option<&'a str>,
}

impl Author<'_> {
    /// Written inside an agent session; a blank session counts as none.
    #[must_use]
    pub fn in_session(self) -> bool {
        !self.by_person
            && self
                .session
                .is_some_and(|session| !session.trim().is_empty())
    }
}

pub(crate) fn author_invalid(error: &BoardError) -> AppError {
    let text = match error {
        BoardError::ActorName(text) | BoardError::Name(text) => *text,
    };
    AppError::text(text, "author-empty", error.to_string())
}

/// The named author, a session inside one and a human otherwise; else the acting person.
pub(crate) async fn resolve(
    actors: &dyn ActorRepository,
    ids: &dyn IdGenerator,
    clock: &dyn Clock,
    author: Author<'_>,
    person: &Actor,
) -> Result<Actor, AppError> {
    let named = author.name.filter(|name| !name.trim().is_empty());
    // legacy context.actor: a session without a name writes under the person's name
    let named = named.or_else(|| author.in_session().then(|| person.name().as_str()));
    let (Some(name), false) = (named, author.by_person) else {
        return Ok(person.clone());
    };
    let name = ActorName::parse(name).map_err(|error| author_invalid(&error))?;
    let kind = if author.in_session() {
        ActorKind::Session
    } else {
        ActorKind::Human
    };
    let candidate = Actor::new(Id::from_uuid(ids.next_uuid()), kind, name, clock.now());
    actors.ensure(&candidate).await
}

#[cfg(test)]
mod tests {
    use nullradix_domain::TextError;

    use super::*;

    #[test]
    fn only_tokens_with_a_filled_session_write_in_one() {
        let named = |by_person, session| Author {
            by_person,
            name: Some("claude"),
            session,
        };
        assert!(named(false, Some("s-1")).in_session());
        assert!(!named(false, Some("  ")).in_session());
        assert!(!named(false, None).in_session());
        assert!(!named(true, Some("s-1")).in_session());
        assert_eq!(
            author_invalid(&BoardError::ActorName(TextError::TooLong(100))).code(),
            "value-too-long"
        );
    }
}
