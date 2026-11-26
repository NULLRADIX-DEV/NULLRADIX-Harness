//! The review before merge: a finished worktree waits for a person, with the tests the agent ran.

use strum::{EnumString, IntoStaticStr};

use crate::{Actor, HUMAN_INTERACTION, Id, Run, Timestamp, Version, Worktree};

/// Block reason of a ticket that waits for a review before the merge.
pub const REVIEW_REASON: &str = "review before merge";

/// The block reason as written on the ticket; it waits for a person.
#[must_use]
pub fn review_reason() -> String {
    format!("{HUMAN_INTERACTION} {REVIEW_REASON}")
}

/// Most test runs a review keeps.
pub const REVIEW_TESTS_MAX: usize = 50;
/// Most characters of a test command a review keeps; a longer one ends in an ellipsis.
pub const REVIEW_COMMAND_MAX: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum ReviewState {
    Pending,
    Approved,
    Returned,
}

impl ReviewState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// A command the agent ran to check its work and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRun {
    pub command: String,
    /// `None` when the call has no result.
    pub ok: Option<bool>,
    pub output: String,
}

impl TestRun {
    /// A test run whose command is cut to [`REVIEW_COMMAND_MAX`] characters.
    #[must_use]
    pub fn new(command: &str, ok: Option<bool>, output: String) -> Self {
        let command = if command.chars().count() > REVIEW_COMMAND_MAX {
            let mut cut: String = command.chars().take(REVIEW_COMMAND_MAX - 1).collect();
            cut.push('\u{2026}');
            cut
        } else {
            command.to_owned()
        };
        Self {
            command,
            ok,
            output,
        }
    }
}

/// Every stored field of a review, for persistence adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRecord {
    pub id: Id<Review>,
    pub worktree: Id<Worktree>,
    pub run: Option<Id<Run>>,
    pub branch: String,
    pub state: ReviewState,
    pub decided_by: Option<Id<Actor>>,
    pub decided_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub version: Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review(ReviewRecord);

impl Review {
    /// A review that waits for a person since `at`.
    #[must_use]
    pub const fn pending(
        id: Id<Self>,
        worktree: Id<Worktree>,
        run: Option<Id<Run>>,
        branch: String,
        at: Timestamp,
    ) -> Self {
        Self(ReviewRecord {
            id,
            worktree,
            run,
            branch,
            state: ReviewState::Pending,
            decided_by: None,
            decided_at: None,
            created_at: at,
            version: Version::INITIAL,
        })
    }

    /// Trusts a record read back from storage.
    #[must_use]
    pub const fn restore(record: ReviewRecord) -> Self {
        Self(record)
    }

    #[must_use]
    pub const fn record(&self) -> &ReviewRecord {
        &self.0
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.0.id
    }
}

/// "x green, y red[, z without result]" or "none found".
#[must_use]
pub fn test_summary(tests: &[TestRun]) -> String {
    if tests.is_empty() {
        return String::from("none found");
    }
    let green = tests.iter().filter(|test| test.ok == Some(true)).count();
    let red = tests.iter().filter(|test| test.ok == Some(false)).count();
    let none = tests.len() - green - red;
    let rest = if none > 0 {
        format!(", {none} without result")
    } else {
        String::new()
    };
    format!("{green} green, {red} red{rest}")
}

/// "n files changed (+a/-d)".
#[must_use]
pub fn change_summary(files: usize, additions: u64, deletions: u64) -> String {
    let word = if files == 1 { "file" } else { "files" };
    format!("{files} {word} changed (+{additions}/-{deletions})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test(ok: Option<bool>) -> TestRun {
        TestRun {
            command: String::from("npm test"),
            ok,
            output: String::new(),
        }
    }

    #[test]
    fn a_command_over_the_limit_is_cut_with_a_mark() {
        let exact = "c".repeat(REVIEW_COMMAND_MAX);
        assert_eq!(TestRun::new(&exact, None, String::new()).command, exact);
        let cut = TestRun::new(&format!("{exact}x"), None, String::new()).command;
        assert_eq!(cut.chars().count(), REVIEW_COMMAND_MAX);
        assert!(cut.starts_with("ccc") && cut.ends_with('\u{2026}'));
    }

    #[test]
    fn the_reason_waits_for_a_person() {
        assert!(crate::waits_for_a_person(Some(&review_reason())));
    }

    #[test]
    fn summarizes_the_test_runs() {
        assert_eq!(test_summary(&[]), "none found");
        assert_eq!(
            test_summary(&[test(Some(true)), test(Some(false))]),
            "1 green, 1 red"
        );
        assert_eq!(
            test_summary(&[test(Some(true)), test(None)]),
            "1 green, 0 red, 1 without result"
        );
    }

    #[test]
    fn counts_files_in_the_singular_too() {
        assert_eq!(change_summary(1, 2, 0), "1 file changed (+2/-0)");
        assert_eq!(change_summary(3, 10, 4), "3 files changed (+10/-4)");
    }

    #[test]
    fn a_new_review_is_pending() {
        let id = |value| uuid::Uuid::from_u128(value);
        let review = Review::pending(
            Id::from_uuid(id(1)),
            Id::from_uuid(id(2)),
            None,
            String::from("ticket/x"),
            Timestamp::from_millis(5),
        );
        assert_eq!(review.record().state, ReviewState::Pending);
        assert_eq!(review.record().version, Version::INITIAL);
        assert_eq!(ReviewState::Pending.as_str(), "pending");
    }
}
