use std::time::Duration;

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
    /// Branch of the worktree the ticket is linked to right now.
    pub branch: Option<String>,
    /// Folder of that worktree; set and cleared together with `branch`.
    pub worktree: Option<String>,
    /// Who wrote the plan and when.
    pub planned: Option<(String, Timestamp)>,
    /// Who plans the ticket right now and since when; a claim lives 15 minutes.
    pub planning_claim: Option<(String, Timestamp)>,
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
            branch: None,
            worktree: None,
            planned: None,
            planning_claim: None,
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

impl TicketRecord {
    /// Ends the link to the worktree; the worktree keeps the ticket in its list.
    pub fn clear_link(&mut self) {
        self.branch = None;
        self.worktree = None;
    }
}

/// A started ticket keeps its target; the lock looks at the branch before the change.
pub fn check_target_lock(before: &TicketRecord, after: &TicketRecord) -> Result<(), TicketError> {
    match &before.branch {
        Some(branch) if after.target_branch != before.target_branch => {
            Err(TicketError::TargetBranchLocked {
                key: before.key.to_string(),
                branch: branch.clone(),
            })
        }
        _ => Ok(()),
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
    let name = |value: &Option<(String, Timestamp)>| value.as_ref().map(|(name, _)| name.clone());
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
        ("plannedBy", name(&before.planned), name(&after.planned)),
        (
            "planningClaimBy",
            name(&before.planning_claim),
            name(&after.planning_claim),
        ),
        ("branch", before.branch.clone(), after.branch.clone()),
        ("worktree", before.worktree.clone(), after.worktree.clone()),
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
    ];
    let mut changes = Vec::new();
    for (field, from, to) in pairs {
        if from != to {
            changes.push(FieldChange { field, from, to });
        }
    }
    // long texts compare by content but show only their length
    let texts = [
        ("description", &before.description, &after.description),
        ("plan", &before.plan, &after.plan),
        ("handbook", &before.handbook, &after.handbook),
        ("diagram", &before.diagram, &after.diagram),
    ];
    for (field, from, to) in texts {
        if from != to {
            changes.push(FieldChange {
                field,
                from: length(from),
                to: length(to),
            });
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
                '\u{8}' => json.push_str("\\b"),
                '\u{c}' => json.push_str("\\f"),
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

/// How long a planning claim keeps others off a ticket.
pub const PLANNING_CLAIM_TTL: Duration = Duration::from_mins(15);

/// Whether someone still holds the planning claim at `now`.
#[must_use]
pub fn planning_claim_alive(record: &TicketRecord, now: Timestamp) -> bool {
    record
        .planning_claim
        .as_ref()
        .is_some_and(|(_, at)| now.saturating_since(*at) < PLANNING_CLAIM_TTL)
}

/// Needs planning, is neither in planning nor not needed, and nobody holds a live claim.
#[must_use]
pub fn planning_candidate(record: &TicketRecord, now: Timestamp) -> bool {
    record.needs_planning
        && !matches!(
            record.status,
            TicketStatus::InPlanning | TicketStatus::NotNeeded
        )
        && !planning_claim_alive(record, now)
}

/// Prefix of a block reason that waits for a person, such as a review before merge.
pub const HUMAN_INTERACTION: &str = "Human Interaction:";

/// Whether the block reason asks a person to accept the ticket.
#[must_use]
pub fn waits_for_a_person(reason: Option<&str>) -> bool {
    let text = reason.unwrap_or_default().trim();
    text.starts_with(HUMAN_INTERACTION) || Some(text) == HUMAN_INTERACTION.strip_suffix(':')
}

/// Blocked on a person, such as a review before the merge.
#[must_use]
pub fn waits_on_a_person(record: &TicketRecord) -> bool {
    record.status == TicketStatus::Blocked && waits_for_a_person(record.blocked_reason.as_deref())
}

/// What a write made by an agent may not do: end a wait for the person or bypass one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentWriteRefusal {
    #[error("moving a ticket that waits for a person")]
    EndsWait,
    #[error("removing the label {0}")]
    DropsLabel(String),
}

/// The one rule for every write an agent makes; `keep_label` is the label that keeps a person in.
pub fn check_agent_write(
    before: &TicketRecord,
    after: &TicketRecord,
    keep_label: Option<&str>,
) -> Result<(), AgentWriteRefusal> {
    if waits_on_a_person(before) && !waits_on_a_person(after) {
        return Err(AgentWriteRefusal::EndsWait);
    }
    let has = |record: &TicketRecord, label: &str| {
        record
            .labels
            .iter()
            .any(|own| own.eq_ignore_ascii_case(label))
    };
    match keep_label {
        Some(label) if has(before, label) && !has(after, label) => {
            Err(AgentWriteRefusal::DropsLabel(label.to_owned()))
        }
        _ => Ok(()),
    }
}

/// A diagram without the Markdown fence around it, as legacy stored plans.
#[must_use]
pub fn strip_fence(diagram: &str) -> String {
    fenced_body(diagram).unwrap_or(diagram).to_owned()
}

fn fenced_body(diagram: &str) -> Option<&str> {
    let rest = diagram.trim_start().strip_prefix("```")?;
    let rest = rest.strip_prefix("mermaid").unwrap_or(rest);
    let inner = rest.trim_end().strip_suffix("```")?;
    let gap = inner.len() - inner.trim_start().len();
    let breaks = |text: &str| -> Vec<usize> {
        text.char_indices()
            .filter(|(_, letter)| *letter == '\n')
            .map(|(at, _)| at)
            .collect()
    };
    // the opening takes as many blank lines as it can and still leaves one for the end
    breaks(&inner[..gap]).into_iter().rev().find_map(|opening| {
        let body = &inner[opening + 1..];
        breaks(body)
            .into_iter()
            .find(|end| body[*end..].trim().is_empty())
            .map(|end| &body[..end])
    })
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
        after.labels = vec![String::from("ui"), String::from("a\"b\u{1}\u{8}")];
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
                ("labels", Some("[]"), Some(r#"["ui","a\"b\u0001\b"]"#)),
                ("dependsOn", Some("[]"), Some(r#"["ARC-0001"]"#)),
                ("description", Some("3 characters"), Some("3 characters")),
                ("plan", Some("0 characters"), Some("2 characters")),
            ]
        );
        assert_eq!(super::changes(before.record(), before.record()), []);
    }

    #[test]
    fn the_link_shows_after_the_claim_and_before_the_target() {
        let mut before = sample().record().clone();
        before.branch = Some(String::from("ticket/ARC-0007-fix-it"));
        before.worktree = Some(String::from("/work/arches-worktrees/ARC-0007"));
        let mut after = before.clone();
        after.clear_link();
        after.planning_claim = Some((String::from("planner"), Timestamp::from_millis(1)));
        after.target_branch = TargetBranch::parse("release/1").unwrap();
        let changes = changes(&before, &after);
        let shown: Vec<(&str, Option<&str>, Option<&str>)> = changes
            .iter()
            .map(|change| (change.field, change.from.as_deref(), change.to.as_deref()))
            .collect();
        assert_eq!(
            shown,
            [
                ("planningClaimBy", None, Some("planner")),
                ("branch", Some("ticket/ARC-0007-fix-it"), None),
                ("worktree", Some("/work/arches-worktrees/ARC-0007"), None),
                ("targetBranch", None, Some("release/1")),
            ]
        );
    }

    #[test]
    fn the_target_locks_once_the_ticket_had_a_branch_before_the_change() {
        let mut before = sample().record().clone();
        let mut after = before.clone();
        after.branch = Some(String::from("ticket/ARC-0007-fix-it"));
        after.target_branch = TargetBranch::parse("release/1").unwrap();
        assert_eq!(check_target_lock(&before, &after), Ok(()));
        before.branch.clone_from(&after.branch);
        assert_eq!(
            check_target_lock(&before, &after),
            Err(TicketError::TargetBranchLocked {
                key: String::from("ARC-0007"),
                branch: String::from("ticket/ARC-0007-fix-it"),
            })
        );
        // clearing in the same change does not lift the lock, an unchanged target passes
        after.clear_link();
        assert!(check_target_lock(&before, &after).is_err());
        after.target_branch = None;
        assert_eq!(check_target_lock(&before, &after), Ok(()));
    }

    #[test]
    fn fences_come_off_diagrams_as_legacy_stripped_them() {
        let cases = [
            ("```mermaid\ngraph TD\n  A-->B\n```", "graph TD\n  A-->B"),
            ("  ```\n\nA \n\n```  ", "A "),
            ("```\n\n```", ""),
            ("graph TD", "graph TD"),
            ("```mermaidx\nA\n```", "```mermaidx\nA\n```"),
            ("```\nA```", "```\nA```"),
        ];
        for (given, stored) in cases {
            assert_eq!(strip_fence(given), stored, "{given:?}");
        }
    }

    #[test]
    fn planning_claims_live_fifteen_minutes() {
        let mut record = sample().record().clone();
        let start = Timestamp::from_millis(1_000);
        assert!(!planning_claim_alive(&record, start));
        record.planning_claim = Some((String::from("planner"), start));
        let later = |minutes: u64| start.checked_add(Duration::from_mins(minutes)).unwrap();
        assert!(planning_claim_alive(&record, later(14)));
        assert!(!planning_claim_alive(&record, later(15)));
    }

    #[test]
    fn an_agent_may_neither_end_a_wait_for_a_person_nor_drop_the_label_that_keeps_one() {
        let open = sample().record().clone();
        let mut waiting = open.clone();
        waiting.status = TicketStatus::Blocked;
        waiting.blocked_reason = Some(String::from("Human Interaction: review"));
        // entering a wait, staying in it and ordinary changes are fine
        assert_eq!(check_agent_write(&open, &waiting, None), Ok(()));
        assert_eq!(check_agent_write(&waiting, &waiting, None), Ok(()));
        assert_eq!(check_agent_write(&open, &open, None), Ok(()));
        // leaving it is not, whatever the next state is
        for status in [
            TicketStatus::Done,
            TicketStatus::Planned,
            TicketStatus::InProgress,
        ] {
            let mut after = waiting.clone();
            after.status = status;
            assert_eq!(
                check_agent_write(&waiting, &after, None),
                Err(AgentWriteRefusal::EndsWait),
                "{status:?}"
            );
        }
        let mut other_reason = waiting.clone();
        other_reason.blocked_reason = Some(String::from("waiting for a key"));
        assert_eq!(
            check_agent_write(&waiting, &other_reason, None),
            Err(AgentWriteRefusal::EndsWait)
        );
        let mut tagged = open.clone();
        tagged.labels = vec![String::from("Interactive"), String::from("ui")];
        let mut dropped = tagged.clone();
        dropped.labels = vec![String::from("ui")];
        assert_eq!(
            check_agent_write(&tagged, &dropped, Some("interactive")),
            Err(AgentWriteRefusal::DropsLabel(String::from("interactive")))
        );
        assert_eq!(check_agent_write(&tagged, &dropped, None), Ok(()));
        assert_eq!(
            check_agent_write(&dropped, &tagged, Some("interactive")),
            Ok(())
        );
        assert_eq!(
            check_agent_write(&dropped, &dropped, Some("interactive")),
            Ok(())
        );
    }

    #[test]
    fn only_human_interaction_reasons_wait_for_a_person() {
        assert!(waits_for_a_person(Some(" Human Interaction: review ")));
        assert!(waits_for_a_person(Some("Human Interaction")));
        assert!(!waits_for_a_person(Some("Merge: conflict")));
        assert!(!waits_for_a_person(None));
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
