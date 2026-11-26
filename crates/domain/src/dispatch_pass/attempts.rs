use crate::{Actor, CommentRecord, Id};

/// How the dispatcher's abort comments begin; attempts are counted from them.
pub const ABORT_PREFIX: &str = "[dispatch] abort";

/// Failed automatic attempts: the dispatcher's comments that begin with the abort marker.
#[must_use]
pub fn failed_attempts<'a>(
    comments: impl IntoIterator<Item = &'a CommentRecord>,
    dispatcher: Id<Actor>,
) -> usize {
    comments
        .into_iter()
        .filter(|comment| {
            comment.author == dispatcher && comment.body.as_str().starts_with(ABORT_PREFIX)
        })
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
}
