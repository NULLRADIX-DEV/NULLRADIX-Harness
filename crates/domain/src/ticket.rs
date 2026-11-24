use crate::{
    Actor, Board, Id, Priority, Project, TicketError, TicketKey, TicketStatus, Timestamp, Version,
    required_text, utf16_len,
};

pub const TITLE_MAX: usize = 400;
pub const BRANCH_MAX: usize = 400;
pub const ASSIGNEE_MAX: usize = 100;
pub const BLOCKED_REASON_MAX: usize = 500;

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

    /// The next version of this ticket with `after`'s fields; identity and creation stay.
    #[must_use]
    pub fn revised(&self, after: TicketRecord, now: Timestamp) -> Self {
        let before = &self.0;
        Self(TicketRecord {
            id: before.id,
            key: before.key.clone(),
            project: before.project,
            created_by: before.created_by,
            created_at: before.created_at,
            updated_at: now,
            version: before.version.next(),
            ..after
        })
    }
}

/// One field that differs between two versions of a ticket, as its history shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChange {
    pub field: &'static str,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Changed fields in the legacy order; lists show as JSON, long texts only by their length.
#[must_use]
pub fn changes(before: &TicketRecord, after: &TicketRecord) -> Vec<FieldChange> {
    let text = |value: &str| Some(value.to_owned());
    let flag = |value: bool| text(if value { "true" } else { "false" });
    let branch = |value: &Option<TargetBranch>| value.as_ref().map(|name| name.0.clone());
    let keys = |keys: &[TicketKey]| keys.iter().map(ToString::to_string).collect::<Vec<_>>();
    let length = |value: &str| Some(format!("{} characters", utf16_len(value)));
    let pairs = [
        (
            "title",
            text(before.title.as_str()),
            text(after.title.as_str()),
        ),
        (
            "status",
            text(before.status.as_str()),
            text(after.status.as_str()),
        ),
        (
            "priority",
            text(before.priority.as_str()),
            text(after.priority.as_str()),
        ),
        ("assignee", before.assignee.clone(), after.assignee.clone()),
        (
            "needsPlanning",
            flag(before.needs_planning),
            flag(after.needs_planning),
        ),
        (
            "acceptance",
            text(&before.acceptance),
            text(&after.acceptance),
        ),
        (
            "blockedReason",
            before.blocked_reason.clone(),
            after.blocked_reason.clone(),
        ),
        (
            "boardId",
            text(&before.board.to_string()),
            text(&after.board.to_string()),
        ),
        (
            "targetBranch",
            branch(&before.target_branch),
            branch(&after.target_branch),
        ),
        (
            "labels",
            Some(json_list(&before.labels)),
            Some(json_list(&after.labels)),
        ),
        (
            "dependsOn",
            Some(json_list(&keys(&before.depends_on))),
            Some(json_list(&keys(&after.depends_on))),
        ),
        (
            "description",
            length(&before.description),
            length(&after.description),
        ),
        ("plan", length(&before.plan), length(&after.plan)),
        (
            "handbook",
            length(&before.handbook),
            length(&after.handbook),
        ),
        ("diagram", length(&before.diagram), length(&after.diagram)),
    ];
    let mut changes = Vec::new();
    for (field, from, to) in pairs {
        if from != to {
            changes.push(FieldChange { field, from, to });
        }
    }
    changes
}

/// A list of texts as a JSON array, as legacy wrote list changes into the history.
fn json_list(items: &[String]) -> String {
    let mut json = String::from("[");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push('"');
        for letter in item.chars() {
            match letter {
                '"' => json.push_str("\\\""),
                '\\' => json.push_str("\\\\"),
                '\n' => json.push_str("\\n"),
                '\r' => json.push_str("\\r"),
                '\t' => json.push_str("\\t"),
                control if u32::from(control) < 0x20 => {
                    let code = u32::from(control);
                    json.push_str("\\u00");
                    json.push(char::from_digit(code >> 4, 16).unwrap_or('0'));
                    json.push(char::from_digit(code & 0xf, 16).unwrap_or('0'));
                }
                other => json.push(other),
            }
        }
        json.push('"');
    }
    json.push(']');
    json
}

/// What stays of a deleted ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    pub id: Id<Self>,
    pub ticket: Id<Ticket>,
    pub project: Id<Project>,
    pub key: TicketKey,
    pub title: String,
    pub from_status: TicketStatus,
    pub actor: Id<Actor>,
    pub at: Timestamp,
}

impl Tombstone {
    #[must_use]
    pub fn of(id: Id<Self>, ticket: &Ticket, actor: Id<Actor>, at: Timestamp) -> Self {
        let record = ticket.record();
        Self {
            id,
            ticket: record.id,
            project: record.project,
            key: record.key.clone(),
            title: record.title.as_str().to_owned(),
            from_status: record.status,
            actor,
            at,
        }
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
    #[must_use]
    pub fn of(
        id: Id<Self>,
        ticket: Id<Ticket>,
        actor: Id<Actor>,
        at: Timestamp,
        change: FieldChange,
        note: Option<&str>,
    ) -> Self {
        Self {
            id,
            ticket,
            actor,
            at,
            field: change.field,
            from: change.from,
            to: change.to,
            note: note.map(str::to_owned),
        }
    }

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

    fn sample() -> Ticket {
        let board = Board::new(
            id(1),
            id(2),
            BoardName::parse("Main").unwrap(),
            id(3),
            Timestamp::from_millis(5),
        );
        let draft = TicketDraft {
            title: TicketTitle::parse("Fix it").unwrap(),
            description: String::from("abc"),
            acceptance: String::new(),
            status: TicketStatus::Backlog,
            priority: Priority::Normal,
            labels: Vec::new(),
            depends_on: Vec::new(),
            needs_planning: true,
            target_branch: None,
        };
        let key = TicketKey::parse("ARC-7").unwrap();
        Ticket::new(
            id(4),
            key,
            &board,
            draft,
            0,
            id(3),
            Timestamp::from_millis(9),
        )
    }

    #[test]
    fn revisions_keep_identity_and_bump_the_version() {
        let before = sample();
        let mut after = before.record().clone();
        after.key = TicketKey::parse("ZED-1").unwrap();
        after.title = TicketTitle::parse("Fixed").unwrap();
        let revised = before.revised(after, Timestamp::from_millis(20));
        let record = revised.record();
        assert_eq!(record.key, before.record().key);
        assert_eq!(record.title.as_str(), "Fixed");
        assert_eq!(record.version, Version::INITIAL.next());
        assert_eq!(
            (record.created_at, record.updated_at),
            (Timestamp::from_millis(9), Timestamp::from_millis(20))
        );
    }

    #[test]
    fn changes_follow_the_legacy_history_format() {
        let before = sample();
        let mut after = before.record().clone();
        after.status = TicketStatus::Planned;
        after.needs_planning = false;
        after.labels = vec![String::from("ui"), String::from("a\"b\u{1}")];
        after.depends_on = vec![TicketKey::parse("ARC-1").unwrap()];
        after.description = String::from("xyz");
        after.plan = String::from("\u{1f600}");
        after.assignee = Some(String::from("claude"));
        let changes = changes(before.record(), &after);
        let shown: Vec<(&str, Option<&str>, Option<&str>)> = changes
            .iter()
            .map(|change| (change.field, change.from.as_deref(), change.to.as_deref()))
            .collect();
        assert_eq!(
            shown,
            [
                ("status", Some("backlog"), Some("planned")),
                ("assignee", None, Some("claude")),
                ("needsPlanning", Some("true"), Some("false")),
                ("labels", Some("[]"), Some(r#"["ui","a\"b\u0001"]"#)),
                ("dependsOn", Some("[]"), Some(r#"["ARC-0001"]"#)),
                ("plan", Some("0 characters"), Some("2 characters")),
            ]
        );
        assert_eq!(super::changes(before.record(), before.record()), []);
    }

    #[test]
    fn tombstones_keep_key_title_and_status() {
        let ticket = sample();
        let tombstone = Tombstone::of(id(8), &ticket, id(3), Timestamp::from_millis(30));
        assert_eq!(tombstone.key.to_string(), "ARC-0007");
        assert_eq!(
            (tombstone.title.as_str(), tombstone.from_status),
            ("Fix it", TicketStatus::Backlog)
        );
    }
}
