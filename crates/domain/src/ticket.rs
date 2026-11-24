use crate::{
    Actor, Board, Id, Priority, Project, TicketError, TicketKey, TicketStatus, Timestamp, Version,
    required_text, utf16_len,
};

pub const TITLE_MAX: usize = 400;
pub const BRANCH_MAX: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketTitle(String);

impl TicketTitle {
    pub fn parse(text: &str) -> Result<Self, TicketError> {
        required_text(text, TITLE_MAX)
            .map(|title| Self(title.to_owned()))
            .map_err(TicketError::Title)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The title as the duplicate check compares it: case does not count.
    #[must_use]
    pub fn folded(&self) -> String {
        self.0.to_lowercase()
    }
}

/// The branch a ticket merges into; none means the default branch of the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetBranch(String);

impl TargetBranch {
    /// Blank text is no target; the rules follow `git check-ref-format --branch`.
    pub fn parse(text: &str) -> Result<Option<Self>, TicketError> {
        let name = text.trim();
        if name.is_empty() {
            return Ok(None);
        }
        if utf16_len(name) > BRANCH_MAX {
            return Err(TicketError::TargetBranchTooLong(BRANCH_MAX));
        }
        if !is_branch_name(name) {
            return Err(TicketError::InvalidTargetBranch(name.to_owned()));
        }
        // `ticket` would clash with refs/heads/ticket/*, and Windows ignores the case
        let lower = name.to_lowercase();
        if lower == "ticket" || lower.starts_with("ticket/") {
            return Err(TicketError::TicketBranchTarget(name.to_owned()));
        }
        Ok(Some(Self(name.to_owned())))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "git refuses only the exact .lock suffix"
)]
fn is_branch_name(name: &str) -> bool {
    if name.starts_with('-') || name == "@" || name == "HEAD" || name.ends_with('.') {
        return false;
    }
    let forbidden = name.chars().any(|letter| {
        letter <= ' '
            || letter == '\u{7f}'
            || matches!(letter, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    });
    if forbidden || name.contains("..") || name.contains("@{") {
        return false;
    }
    name.split('/')
        .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// What a new ticket brings besides the defaults every ticket starts with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketDraft {
    pub title: TicketTitle,
    pub description: String,
    pub acceptance: String,
    pub status: TicketStatus,
    pub priority: Priority,
    /// Already normalized by [`crate::normalize_labels`].
    pub labels: Vec<String>,
    /// Already normalized by [`crate::normalize_dependencies`] and validated.
    pub depends_on: Vec<TicketKey>,
    pub needs_planning: bool,
    pub target_branch: Option<TargetBranch>,
}

/// Every stored field of a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRecord {
    pub id: Id<Ticket>,
    pub key: TicketKey,
    pub project: Id<Project>,
    pub board: Id<Board>,
    pub title: TicketTitle,
    pub description: String,
    pub acceptance: String,
    pub plan: String,
    pub handbook: String,
    pub diagram: String,
    pub status: TicketStatus,
    pub priority: Priority,
    pub labels: Vec<String>,
    pub depends_on: Vec<TicketKey>,
    /// Place at the end of its column when it got there; the display order ignores it.
    pub sort_order: i64,
    pub assignee: Option<String>,
    pub needs_planning: bool,
    pub blocked_reason: Option<String>,
    pub target_branch: Option<TargetBranch>,
    pub created_by: Id<Actor>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket(TicketRecord);

impl Ticket {
    /// A ticket placed on `board` at the end of its column, as legacy created them.
    #[must_use]
    pub fn new(
        id: Id<Self>,
        key: TicketKey,
        board: &Board,
        draft: TicketDraft,
        sort_order: i64,
        created_by: Id<Actor>,
        now: Timestamp,
    ) -> Self {
        Self(TicketRecord {
            id,
            key,
            project: board.project(),
            board: board.id(),
            title: draft.title,
            description: draft.description,
            acceptance: draft.acceptance,
            plan: String::new(),
            handbook: String::new(),
            diagram: String::new(),
            status: draft.status,
            priority: draft.priority,
            labels: draft.labels,
            depends_on: draft.depends_on,
            sort_order,
            assignee: None,
            needs_planning: draft.needs_planning,
            blocked_reason: None,
            target_branch: draft.target_branch,
            created_by,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        })
    }

    #[must_use]
    pub const fn restore(record: TicketRecord) -> Self {
        Self(record)
    }

    #[must_use]
    pub const fn record(&self) -> &TicketRecord {
        &self.0
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.0.id
    }

    #[must_use]
    pub const fn key(&self) -> &TicketKey {
        &self.0.key
    }

    #[must_use]
    pub const fn status(&self) -> TicketStatus {
        self.0.status
    }
}

/// One line of a ticket's history: a field that changed, or a note such as `created`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: Id<Self>,
    pub ticket: Id<Ticket>,
    pub actor: Id<Actor>,
    pub at: Timestamp,
    pub field: &'static str,
    pub from: Option<String>,
    pub to: Option<String>,
    pub note: Option<String>,
}

impl HistoryEntry {
    /// The first entry of every ticket: its status from nothing, noted as created.
    #[must_use]
    pub fn created(id: Id<Self>, ticket: &Ticket) -> Self {
        let record = ticket.record();
        Self {
            id,
            ticket: record.id,
            actor: record.created_by,
            at: record.created_at,
            field: "status",
            from: None,
            to: Some(record.status.as_str().to_owned()),
            note: Some(String::from("created")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BoardName, TextError};

    fn id<T>(value: u128) -> Id<T> {
        Id::from_uuid(uuid::Uuid::from_u128(value))
    }

    #[test]
    fn titles_are_trimmed_bounded_and_fold_their_case() {
        assert_eq!(TicketTitle::parse("  Fix it ").unwrap().as_str(), "Fix it");
        assert_eq!(
            TicketTitle::parse(" "),
            Err(TicketError::Title(TextError::Empty))
        );
        assert_eq!(
            TicketTitle::parse(&"x".repeat(401)),
            Err(TicketError::Title(TextError::TooLong(400)))
        );
        assert_eq!(
            TicketTitle::parse("\u{c4}rger").unwrap().folded(),
            "\u{e4}rger"
        );
    }

    #[test]
    fn target_branches_follow_git_rules_and_skip_ticket_branches() {
        assert_eq!(TargetBranch::parse("  "), Ok(None));
        assert_eq!(
            TargetBranch::parse(" release/2.0 ")
                .unwrap()
                .unwrap()
                .as_str(),
            "release/2.0"
        );
        for bad in [
            "-x", "@", "HEAD", "a..b", "a b", "a~1", "x.", "a//b", "/a", "a/.b", "a.lock", "a@{1",
            "a\\b",
        ] {
            assert_eq!(
                TargetBranch::parse(bad),
                Err(TicketError::InvalidTargetBranch(bad.to_owned())),
                "{bad}"
            );
        }
        for taken in ["ticket", "Ticket/ARC-1"] {
            assert_eq!(
                TargetBranch::parse(taken),
                Err(TicketError::TicketBranchTarget(taken.to_owned()))
            );
        }
        assert_eq!(
            TargetBranch::parse(&"b".repeat(401)),
            Err(TicketError::TargetBranchTooLong(400))
        );
    }

    #[test]
    fn new_tickets_start_on_the_board_with_one_created_entry() {
        let board = Board::new(
            id(1),
            id(2),
            BoardName::parse("Main").unwrap(),
            id(3),
            Timestamp::from_millis(5),
        );
        let draft = TicketDraft {
            title: TicketTitle::parse("Fix it").unwrap(),
            description: String::new(),
            acceptance: String::new(),
            status: TicketStatus::Planned,
            priority: Priority::High,
            labels: vec![String::from("ui")],
            depends_on: Vec::new(),
            needs_planning: true,
            target_branch: None,
        };
        let key = TicketKey::parse("ARC-7").unwrap();
        let ticket = Ticket::new(
            id(4),
            key.clone(),
            &board,
            draft,
            3,
            id(3),
            Timestamp::from_millis(9),
        );
        let record = ticket.record();
        assert_eq!((record.project, record.board), (id(2), id(1)));
        assert_eq!(record.version, Version::INITIAL);
        assert_eq!(record.updated_at, record.created_at);
        assert_eq!(
            (ticket.key(), ticket.status()),
            (&key, TicketStatus::Planned)
        );

        let entry = HistoryEntry::created(id(5), &ticket);
        assert_eq!(entry.to.as_deref(), Some("planned"));
        assert_eq!(
            (entry.field, entry.from, entry.note.as_deref()),
            ("status", None, Some("created"))
        );
    }
}
