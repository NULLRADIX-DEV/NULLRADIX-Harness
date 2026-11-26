use crate::{Actor, CommentRecord, Id, Run};

/// How the dispatcher's abort comments begin; attempts are counted from them.
pub const ABORT_PREFIX: &str = "[dispatch] abort";

/// Whether the comment is an abort comment of the dispatcher.
#[must_use]
pub fn is_abort(comment: &CommentRecord, dispatcher: Id<Actor>) -> bool {
    comment.author == dispatcher && comment.body.as_str().starts_with(ABORT_PREFIX)
}

/// Whether the comment is the abort comment the dispatcher wrote for that run.
#[must_use]
pub fn is_abort_of(comment: &CommentRecord, dispatcher: Id<Actor>, run: Id<Run>) -> bool {
    is_abort(comment, dispatcher) && comment.run == Some(run)
}

/// Failed automatic attempts: the dispatcher's comments that begin with the abort marker.
#[must_use]
pub fn failed_attempts<'a>(
    comments: impl IntoIterator<Item = &'a CommentRecord>,
    dispatcher: Id<Actor>,
) -> usize {
    comments
        .into_iter()
        .filter(|comment| is_abort(comment, dispatcher))
        .count()
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::{CommentBody, CommentOn, Id, Ticket, Timestamp, Version};

    fn id<T>(value: u128) -> Id<T> {
        Id::from_uuid(Uuid::from_u128(value))
    }

    fn comment(author: u128, body: &str) -> CommentRecord {
        CommentRecord {
            id: id(100),
            subject: CommentOn::Ticket(id::<Ticket>(1)),
            author: id(author),
            body: CommentBody::parse(body).unwrap(),
            system_marker: None,
            run: None,
            delivered_at: None,
            created_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(1),
            version: Version::INITIAL,
        }
    }

    #[test]
    fn only_abort_comments_of_the_dispatcher_count() {
        let comments = [
            comment(7, "[dispatch] abort: attempt 1/2: time limit"),
            comment(7, "[dispatch] start: handed to worker-ARC-1"),
            comment(8, "[dispatch] abort: faked by a person"),
            comment(7, "[dispatch] abort: attempt 2/2: exit"),
            comment(7, "not a [dispatch] abort"),
            comment(7, "[dispatch] pause: usage limit until 10:00"),
        ];
        assert_eq!(failed_attempts(&comments, id(7)), 2);
        assert_eq!(failed_attempts(&comments, id(9)), 0);
        assert_eq!(failed_attempts([], id(7)), 0);
    }

    #[test]
    fn an_abort_belongs_to_the_run_it_names() {
        let run = id::<Run>(5);
        let mut own = comment(7, "[dispatch] abort: attempt 1/2: x");
        assert!(is_abort(&own, id(7)));
        assert!(!is_abort_of(&own, id(7), run));
        own.run = Some(run);
        assert!(is_abort_of(&own, id(7), run));
        assert!(!is_abort_of(&own, id(7), id(6)));
        assert!(!is_abort_of(&own, id(8), run));
    }
}
