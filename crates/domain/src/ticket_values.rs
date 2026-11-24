use std::fmt;

use strum::{EnumIter, EnumString, IntoEnumIterator, IntoStaticStr};

use crate::{ProjectKey, TextError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TicketError {
    #[error("{0} is no ticket key, expected something like ARC-0042")]
    InvalidKey(String),
    #[error("status {0} is unknown")]
    InvalidStatus(String),
    #[error("waiting comes from open dependencies and is no status")]
    WaitingIsDerived,
    #[error("priority {0} is unknown, allowed are low, normal, high and urgent")]
    InvalidPriority(String),
    #[error("the title {0}")]
    Title(TextError),
    #[error("the target branch has more than {0} characters")]
    TargetBranchTooLong(usize),
    #[error("{0} is no valid branch name")]
    InvalidTargetBranch(String),
    #[error("{0} is the branch of a ticket, nothing merges there")]
    TicketBranchTarget(String),
}

/// The human key of a ticket, such as `ARC-0042`; numbers grow past four digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TicketKey {
    project: ProjectKey,
    number: u32,
}

impl TicketKey {
    #[must_use]
    pub const fn new(project: ProjectKey, number: u32) -> Self {
        Self { project, number }
    }

    /// Accepts any case and unpadded numbers, so `arc-42` is `ARC-0042`.
    pub fn parse(text: &str) -> Result<Self, TicketError> {
        let invalid = || TicketError::InvalidKey(text.to_owned());
        let (key, number) = text.trim().rsplit_once('-').ok_or_else(invalid)?;
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        let number = number.parse::<u32>().map_err(|_| invalid())?;
        let project = ProjectKey::parse(&key.to_ascii_uppercase()).map_err(|_| invalid())?;
        Ok(Self { project, number })
    }

    #[must_use]
    pub const fn project(&self) -> &ProjectKey {
        &self.project
    }

    #[must_use]
    pub const fn number(&self) -> u32 {
        self.number
    }
}

impl fmt::Display for TicketKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{:04}", self.project, self.number)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum TicketStatus {
    Backlog,
    InPlanning,
    Planned,
    InProgress,
    Blocked,
    Done,
    NotNeeded,
}

impl TicketStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// Done and not needed close a ticket; every other status keeps it open.
    #[must_use]
    pub const fn is_open(self) -> bool {
        !matches!(self, Self::Done | Self::NotNeeded)
    }

    /// Reads a status in any case, with spaces or underscores for dashes; waiting is no status.
    pub fn parse(text: &str) -> Result<Self, TicketError> {
        Ok(match Column::parse(text)? {
            Column::Waiting => return Err(TicketError::WaitingIsDerived),
            Column::Backlog => Self::Backlog,
            Column::InPlanning => Self::InPlanning,
            Column::Planned => Self::Planned,
            Column::InProgress => Self::InProgress,
            Column::Blocked => Self::Blocked,
            Column::Done => Self::Done,
            Column::NotNeeded => Self::NotNeeded,
        })
    }
}

/// A board column: the statuses plus waiting, which open dependencies derive.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, IntoStaticStr, EnumString, EnumIter,
)]
#[strum(serialize_all = "kebab-case")]
pub enum Column {
    Backlog,
    Waiting,
    InPlanning,
    Planned,
    InProgress,
    Blocked,
    Done,
    NotNeeded,
}

impl Column {
    /// Only tickets still ahead of the work wait; blocked and later ones never do.
    #[must_use]
    pub const fn of(status: TicketStatus, has_open_dependency: bool) -> Self {
        match status {
            TicketStatus::Backlog | TicketStatus::InPlanning | TicketStatus::Planned
                if has_open_dependency =>
            {
                Self::Waiting
            }
            TicketStatus::Backlog => Self::Backlog,
            TicketStatus::InPlanning => Self::InPlanning,
            TicketStatus::Planned => Self::Planned,
            TicketStatus::InProgress => Self::InProgress,
            TicketStatus::Blocked => Self::Blocked,
            TicketStatus::Done => Self::Done,
            TicketStatus::NotNeeded => Self::NotNeeded,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// Reads a column in any case, with spaces or underscores for dashes.
    pub fn parse(text: &str) -> Result<Self, TicketError> {
        let dashed = text
            .trim()
            .to_lowercase()
            .replace('_', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-");
        Self::iter()
            .find(|column| column.as_str() == dashed)
            .ok_or_else(|| TicketError::InvalidStatus(text.to_owned()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
    Urgent,
}

impl Priority {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// Place in the display order; urgent comes first.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Urgent => 0,
            Self::High => 1,
            Self::Normal => 2,
            Self::Low => 3,
        }
    }

    pub fn parse(text: &str) -> Result<Self, TicketError> {
        text.parse()
            .map_err(|_| TicketError::InvalidPriority(text.to_owned()))
    }
}

/// Why a status change is refused; the order of the checks follows the legacy board.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StatusLock {
    #[error("only a person may decide that a ticket is not needed")]
    NotNeededByPeople,
    #[error("a ticket is done only with acceptance criteria")]
    AcceptanceMissing,
    #[error("dependencies are still open: {}", join(.0))]
    DependencyOpen(Vec<TicketKey>),
    #[error("questions about the ticket are still open: {}", .0.join(", "))]
    QuestionOpen(Vec<String>),
}

fn join(keys: &[TicketKey]) -> String {
    keys.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a status check needs to know about the ticket and its surroundings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusFacts {
    /// True when the acceptance criteria are empty after trimming.
    pub acceptance_blank: bool,
    pub open_dependencies: Vec<TicketKey>,
    /// Human keys of open questions that name the ticket.
    pub open_questions: Vec<String>,
}

/// Who asks for a status change and whether the locks that allow it are lifted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusRequest {
    pub by_person: bool,
    pub force: bool,
}

/// Checks a change of status; staying in the same status is never locked.
pub fn check_status_change(
    from: Option<TicketStatus>,
    to: TicketStatus,
    request: StatusRequest,
    facts: &StatusFacts,
) -> Result<(), StatusLock> {
    if from == Some(to) {
        return Ok(());
    }
    if to == TicketStatus::NotNeeded && !request.by_person && !request.force {
        return Err(StatusLock::NotNeededByPeople);
    }
    // even force cannot finish a ticket nobody can check
    if to == TicketStatus::Done && facts.acceptance_blank {
        return Err(StatusLock::AcceptanceMissing);
    }
    if matches!(to, TicketStatus::InProgress | TicketStatus::Done) && !request.force {
        if !facts.open_dependencies.is_empty() {
            return Err(StatusLock::DependencyOpen(facts.open_dependencies.clone()));
        }
        if !facts.open_questions.is_empty() {
            return Err(StatusLock::QuestionOpen(facts.open_questions.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> TicketKey {
        TicketKey::parse(text).unwrap()
    }

    #[test]
    fn ticket_keys_read_loosely_and_print_padded() {
        assert_eq!(key("arc-42").to_string(), "ARC-0042");
        assert_eq!(key(" ARC-12345 ").to_string(), "ARC-12345");
        assert_eq!(key("A1B2-7").project().as_str(), "A1B2");
        assert_eq!(key("ARC-7").number(), 7);
        for bad in [
            "ARC",
            "ARC-",
            "A-1",
            "ARCDEFG-1",
            "1RC-1",
            "ARC-x1",
            "ARC-99999999999",
            "ARC--1",
        ] {
            assert!(TicketKey::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn project_keys_normalize_like_legacy() {
        assert_eq!(ProjectKey::normalize("Inc-1").unwrap().as_str(), "INC1");
        assert_eq!(ProjectKey::normalize(" arc ").unwrap().as_str(), "ARC");
        assert!(ProjectKey::normalize("a!").is_err());
    }

    #[test]
    fn statuses_and_columns_read_their_english_names_in_any_spelling() {
        assert_eq!(TicketStatus::InProgress.as_str(), "in-progress");
        let cases = [
            ("in-progress", TicketStatus::InProgress),
            (" In Progress ", TicketStatus::InProgress),
            ("not_needed", TicketStatus::NotNeeded),
            ("BACKLOG", TicketStatus::Backlog),
        ];
        for (text, status) in cases {
            assert_eq!(TicketStatus::parse(text), Ok(status), "{text}");
        }
        assert_eq!(
            TicketStatus::parse("Waiting"),
            Err(TicketError::WaitingIsDerived)
        );
        assert_eq!(Column::parse("waiting"), Ok(Column::Waiting));
        // the German legacy aliases are gone, as the terms table decided
        for text in ["later", "in Arbeit", "fertig", "wip", ""] {
            assert_eq!(
                TicketStatus::parse(text),
                Err(TicketError::InvalidStatus(text.to_owned())),
                "{text}"
            );
        }
        assert!(!TicketStatus::Done.is_open() && !TicketStatus::NotNeeded.is_open());
        assert!(TicketStatus::Blocked.is_open());
    }

    #[test]
    fn columns_put_waiting_tickets_right_after_the_backlog() {
        assert_eq!(Column::of(TicketStatus::Planned, true), Column::Waiting);
        assert_eq!(Column::of(TicketStatus::Blocked, true), Column::Blocked);
        assert_eq!(Column::of(TicketStatus::Backlog, false), Column::Backlog);
        let order: Vec<&str> = Column::iter().map(Column::as_str).collect();
        assert_eq!(
            order,
            [
                "backlog",
                "waiting",
                "in-planning",
                "planned",
                "in-progress",
                "blocked",
                "done",
                "not-needed"
            ]
        );
    }

    #[test]
    fn priorities_rank_urgent_first() {
        assert_eq!(Priority::default(), Priority::Normal);
        assert_eq!(Priority::parse("urgent"), Ok(Priority::Urgent));
        assert!(Priority::parse("Urgent").is_err());
        let mut all: Vec<Priority> = Priority::iter().collect();
        all.sort_by_key(|priority| priority.rank());
        assert_eq!(
            all,
            [
                Priority::Urgent,
                Priority::High,
                Priority::Normal,
                Priority::Low
            ]
        );
    }

    #[test]
    fn status_locks_follow_the_legacy_order() {
        let open = StatusFacts {
            acceptance_blank: true,
            open_dependencies: vec![key("ARC-1")],
            open_questions: vec![String::from("ARC-Q0001")],
        };
        let agent = StatusRequest::default();
        let person = StatusRequest {
            by_person: true,
            force: false,
        };
        let forced = StatusRequest {
            by_person: false,
            force: true,
        };
        let check = |to, request, facts: &StatusFacts| {
            check_status_change(Some(TicketStatus::Backlog), to, request, facts)
        };

        assert_eq!(
            check(TicketStatus::NotNeeded, agent, &open),
            Err(StatusLock::NotNeededByPeople)
        );
        assert_eq!(check(TicketStatus::NotNeeded, person, &open), Ok(()));
        assert_eq!(
            check(TicketStatus::Done, person, &open),
            Err(StatusLock::AcceptanceMissing)
        );
        assert_eq!(
            check(TicketStatus::Done, forced, &open),
            Err(StatusLock::AcceptanceMissing)
        );
        assert_eq!(
            check(TicketStatus::InProgress, person, &open),
            Err(StatusLock::DependencyOpen(vec![key("ARC-1")]))
        );
        let questions_only = StatusFacts {
            open_dependencies: Vec::new(),
            ..open.clone()
        };
        assert_eq!(
            check(TicketStatus::InProgress, agent, &questions_only),
            Err(StatusLock::QuestionOpen(vec![String::from("ARC-Q0001")]))
        );
        assert_eq!(check(TicketStatus::InProgress, forced, &open), Ok(()));
        assert_eq!(check(TicketStatus::Blocked, agent, &open), Ok(()));
        assert_eq!(
            check_status_change(Some(TicketStatus::Done), TicketStatus::Done, agent, &open),
            Ok(())
        );
        assert_eq!(
            check_status_change(None, TicketStatus::InProgress, agent, &open),
            Err(StatusLock::DependencyOpen(vec![key("ARC-1")]))
        );
    }
}
