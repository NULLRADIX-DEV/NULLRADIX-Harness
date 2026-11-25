//! The inbox: what waits for a person, how urgent it is and the order they chose.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;

use strum::{EnumIter, IntoEnumIterator, IntoStaticStr};

use crate::note::shown_title;
use crate::{
    Board, Id, MERGE_REASON, Priority, ProjectKey, QuestionKey, TicketKey, TicketStatus, Timestamp,
    utf16_len, utf16_prefix, waits_for_a_person,
};

/// Prefix of a block reason the worktree setup wrote when a step failed.
pub const SETUP_REASON: &str = "Setup failed:";
/// How many entries a saved order keeps.
pub const INBOX_ORDER_MAX: usize = 1000;
/// Longest title of a note entry.
pub const INBOX_TITLE_MAX: usize = 120;
/// Longest detail of an entry.
pub const INBOX_DETAIL_MAX: usize = 300;

/// Kind of an entry; the order is the urgency, merge first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, IntoStaticStr, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum InboxKind {
    Merge,
    Setup,
    Question,
    Accept,
    Blocked,
    Planning,
    Note,
}

impl InboxKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// Name people read, as legacy `INBOX_KIND_LABELS`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Merge => "Merge failed",
            Self::Setup => "Setup failed",
            Self::Question => "Question",
            Self::Accept => "Acceptance",
            Self::Blocked => "Blocked",
            Self::Planning => "In planning",
            Self::Note => "Note",
        }
    }

    /// Urgency, 1 is the most urgent.
    #[must_use]
    pub const fn rank(self) -> u32 {
        self as u32 + 1
    }

    /// The kinds named as `merge, setup, ...` for help and refusals.
    #[must_use]
    pub fn names() -> String {
        Self::iter()
            .map(Self::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn parse(text: &str) -> Result<Self, InboxError> {
        Self::iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| InboxError::InvalidKind(text.to_owned()))
    }

    /// Kinds split by commas; blank parts are skipped, but at least one must remain.
    pub fn parse_list(text: &str) -> Result<Vec<Self>, InboxError> {
        let names: Vec<&str> = text
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        if names.is_empty() {
            return Err(InboxError::InvalidKind(text.to_owned()));
        }
        names.into_iter().map(Self::parse).collect()
    }

    /// Kind of a ticket from status and block reason; `None` waits for nobody.
    #[must_use]
    pub fn of_ticket(status: TicketStatus, blocked_reason: Option<&str>) -> Option<Self> {
        match status {
            TicketStatus::InPlanning => Some(Self::Planning),
            TicketStatus::Blocked => {
                let reason = blocked_reason.unwrap_or_default().trim();
                Some(if reason.starts_with(MERGE_REASON) {
                    Self::Merge
                } else if reason.starts_with(SETUP_REASON) {
                    Self::Setup
                } else if waits_for_a_person(Some(reason)) {
                    Self::Accept
                } else {
                    Self::Blocked
                })
            }
            _ => None,
        }
    }
}

impl fmt::Display for InboxKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an entry points at; ordered and saved by this.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InboxRef {
    Ticket(TicketKey),
    Question(QuestionKey),
    Note(i64),
}

impl InboxRef {
    /// Reads `ticket`, `question` or `note` with its key, such as `note` and `7`.
    pub fn parse(item_type: &str, key: &str) -> Result<Self, InboxError> {
        let invalid = || InboxError::InvalidKey(format!("{item_type}:{key}"));
        match item_type {
            "ticket" => TicketKey::parse(key)
                .map(Self::Ticket)
                .map_err(|_| invalid()),
            "question" => QuestionKey::parse(key)
                .map(Self::Question)
                .map_err(|_| invalid()),
            "note" => match key.trim().parse::<i64>() {
                Ok(number) if number >= 1 => Ok(Self::Note(number)),
                _ => Err(invalid()),
            },
            _ => Err(invalid()),
        }
    }

    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Ticket(_) => "ticket",
            Self::Question(_) => "question",
            Self::Note(_) => "note",
        }
    }

    /// The key without its type, such as `ARC-0001` or `7`.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Ticket(key) => key.to_string(),
            Self::Question(key) => key.to_string(),
            Self::Note(number) => number.to_string(),
        }
    }
}

impl fmt::Display for InboxRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.type_name(), self.key())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboxError {
    #[error("\"{0}\" is not a kind; allowed: {names}", names = InboxKind::names())]
    InvalidKind(String),
    #[error("\"{0}\" is not an inbox entry such as ticket:ARC-0001, question:ARC-Q0001 or note:7")]
    InvalidKey(String),
}

/// One entry of the inbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxItem {
    pub item: InboxRef,
    pub kind: InboxKind,
    pub title: String,
    /// Block reason, occasion of the question or the rest of the note.
    pub detail: Option<String>,
    pub board: Option<Id<Board>>,
    pub board_name: Option<String>,
    pub project_key: ProjectKey,
    /// Questions only: open tickets waiting for the answer.
    pub waiting_tickets: u32,
    /// Tickets only; orders entries of one kind.
    pub priority: Option<Priority>,
    /// Last change of a ticket, creation of a question or note.
    pub since: Timestamp,
}

impl InboxItem {
    /// Builds an entry with its detail clipped; blank details are none.
    #[must_use]
    pub fn new(item: InboxRef, kind: InboxKind, title: String, project_key: ProjectKey) -> Self {
        Self {
            item,
            kind,
            title,
            detail: None,
            board: None,
            board_name: None,
            project_key,
            waiting_tickets: 0,
            priority: None,
            since: Timestamp::from_millis(0),
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: Option<&str>) -> Self {
        self.detail = detail
            .filter(|detail| !detail.is_empty())
            .map(|detail| clip(detail, INBOX_DETAIL_MAX));
        self
    }

    #[must_use]
    pub const fn rank(&self) -> u32 {
        self.kind.rank()
    }
}

/// Default order: kind, more waiting tickets, higher priority, older, then the key.
#[must_use]
pub fn compare(a: &InboxItem, b: &InboxItem) -> Ordering {
    let priority = |item: &InboxItem| item.priority.unwrap_or_default().rank();
    a.rank()
        .cmp(&b.rank())
        .then_with(|| b.waiting_tickets.cmp(&a.waiting_tickets))
        .then_with(|| priority(a).cmp(&priority(b)))
        .then_with(|| a.since.cmp(&b.since))
        .then_with(|| a.item.to_string().cmp(&b.item.to_string()))
}

/// Entries without a saved place first in their default order, then the saved ones.
#[must_use]
pub fn apply_order(items: Vec<InboxItem>, saved: &[InboxRef]) -> Vec<InboxItem> {
    let mut position: HashMap<&InboxRef, usize> = HashMap::new();
    for (index, item) in saved.iter().enumerate() {
        position.entry(item).or_insert(index);
    }
    let (mut placed, fresh): (Vec<InboxItem>, Vec<InboxItem>) = items
        .into_iter()
        .partition(|item| position.contains_key(&item.item));
    placed.sort_by_key(|item| position.get(&item.item).copied().unwrap_or(usize::MAX));
    fresh.into_iter().chain(placed).collect()
}

/// Entries of each kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InboxCounts {
    pub merge: u32,
    pub setup: u32,
    pub question: u32,
    pub accept: u32,
    pub blocked: u32,
    pub planning: u32,
    pub note: u32,
}

impl InboxCounts {
    #[must_use]
    pub fn of(items: &[InboxItem]) -> Self {
        let mut counts = Self::default();
        for item in items {
            let count = match item.kind {
                InboxKind::Merge => &mut counts.merge,
                InboxKind::Setup => &mut counts.setup,
                InboxKind::Question => &mut counts.question,
                InboxKind::Accept => &mut counts.accept,
                InboxKind::Blocked => &mut counts.blocked,
                InboxKind::Planning => &mut counts.planning,
                InboxKind::Note => &mut counts.note,
            };
            *count += 1;
        }
        counts
    }
}

/// A saved order without repeats, cut to the first [`INBOX_ORDER_MAX`] entries.
#[must_use]
pub fn order(entries: Vec<InboxRef>) -> Vec<InboxRef> {
    let mut seen: HashSet<InboxRef> = HashSet::with_capacity(entries.len());
    let mut unique: Vec<InboxRef> = Vec::with_capacity(entries.len().min(INBOX_ORDER_MAX));
    for entry in entries {
        if unique.len() == INBOX_ORDER_MAX {
            break;
        }
        if seen.insert(entry.clone()) {
            unique.push(entry);
        }
    }
    unique
}

/// Title and detail of an open note: seen count and the body beyond the title.
#[must_use]
pub fn note_text(
    title: &str,
    body: &str,
    number: i64,
    seen_count: usize,
) -> (String, Option<String>) {
    let lines: Vec<&str> = body.trim().lines().collect();
    let skip = usize::from(title.trim().is_empty());
    let rest = lines
        .iter()
        .skip(skip)
        .flat_map(|line| line.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ");
    let seen = if seen_count > 1 {
        format!("seen {seen_count} times")
    } else {
        String::new()
    };
    let detail = [seen.as_str(), rest.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    let shown = clip(&shown_title(title, body, number), INBOX_TITLE_MAX);
    (shown, (!detail.is_empty()).then_some(detail))
}

/// `text` cut to `max` UTF-16 units ending in `...`.
#[must_use]
pub fn clip(text: &str, max: usize) -> String {
    if utf16_len(text) > max {
        format!("{}...", utf16_prefix(text, max.saturating_sub(3)))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectKey {
        ProjectKey::parse("ARC").unwrap()
    }

    fn ticket(number: u32) -> InboxRef {
        InboxRef::Ticket(TicketKey::new(project(), number))
    }

    fn item(reference: InboxRef, kind: InboxKind) -> InboxItem {
        let mut item = InboxItem::new(reference, kind, String::from("t"), project());
        item.since = Timestamp::from_millis(10);
        item
    }

    fn keys(items: &[InboxItem]) -> Vec<String> {
        items.iter().map(|item| item.item.to_string()).collect()
    }

    fn sorted(mut items: Vec<InboxItem>) -> Vec<String> {
        items.sort_by(compare);
        keys(&items)
    }

    #[test]
    fn blocked_tickets_take_their_kind_from_the_reason() {
        let cases = [
            (Some("Merge: conflict in a.rs"), InboxKind::Merge),
            (Some("Setup failed: npm ci"), InboxKind::Setup),
            (Some("Human Interaction: look at it"), InboxKind::Accept),
            (Some("  Human Interaction"), InboxKind::Accept),
            (Some("  Merge: padded"), InboxKind::Merge),
            (Some("no database access"), InboxKind::Blocked),
            (Some("merge: lower case"), InboxKind::Blocked),
            (Some("human interaction: lower case"), InboxKind::Blocked),
            (None, InboxKind::Blocked),
        ];
        for (reason, kind) in cases {
            assert_eq!(
                InboxKind::of_ticket(TicketStatus::Blocked, reason),
                Some(kind),
                "{reason:?}"
            );
        }
    }

    #[test]
    fn only_blocked_and_in_planning_tickets_wait() {
        assert_eq!(
            InboxKind::of_ticket(TicketStatus::InPlanning, Some("Merge: old")),
            Some(InboxKind::Planning)
        );
        for status in [
            TicketStatus::Backlog,
            TicketStatus::Planned,
            TicketStatus::InProgress,
            TicketStatus::Done,
            TicketStatus::NotNeeded,
        ] {
            assert_eq!(InboxKind::of_ticket(status, Some("Merge: old")), None);
        }
    }

    #[test]
    fn kinds_sort_by_urgency_and_parse_from_lists() {
        let mut items: Vec<InboxItem> = InboxKind::iter()
            .rev()
            .zip(1..)
            .map(|(kind, number)| item(ticket(number), kind))
            .collect();
        items.sort_by(compare);
        let kinds: Vec<InboxKind> = items.iter().map(|item| item.kind).collect();
        assert_eq!(kinds, InboxKind::iter().collect::<Vec<_>>());
        assert_eq!(InboxKind::Merge.rank(), 1);
        assert_eq!(InboxKind::Note.rank(), 7);
        assert_eq!(
            InboxKind::parse_list("merge, accept,").unwrap(),
            [InboxKind::Merge, InboxKind::Accept]
        );
        assert_eq!(
            InboxKind::parse_list("nonsense").unwrap_err().to_string(),
            "\"nonsense\" is not a kind; allowed: merge, setup, question, accept, blocked, \
             planning, note"
        );
        assert_eq!(
            InboxKind::parse_list(",").unwrap_err(),
            InboxError::InvalidKind(String::from(","))
        );
    }

    #[test]
    fn ties_go_to_more_waiting_tickets_higher_priority_then_older() {
        let question = |number, waiting| {
            let mut entry = item(
                InboxRef::Question(QuestionKey::new(project(), number)),
                InboxKind::Question,
            );
            entry.waiting_tickets = waiting;
            entry
        };
        assert_eq!(
            sorted(vec![question(1, 1), question(2, 3)]),
            ["question:ARC-Q0002", "question:ARC-Q0001"]
        );
        let planning = |number, priority| {
            let mut entry = item(ticket(number), InboxKind::Planning);
            entry.priority = priority;
            entry
        };
        assert_eq!(
            sorted(vec![
                planning(1, Some(Priority::Low)),
                planning(2, Some(Priority::Urgent)),
                planning(3, None),
            ]),
            ["ticket:ARC-0002", "ticket:ARC-0003", "ticket:ARC-0001"]
        );
        let mut newer = item(ticket(1), InboxKind::Blocked);
        newer.since = Timestamp::from_millis(20);
        let older = item(ticket(2), InboxKind::Blocked);
        assert_eq!(
            sorted(vec![newer, older]),
            ["ticket:ARC-0002", "ticket:ARC-0001"]
        );
    }

    #[test]
    fn saved_order_puts_new_entries_on_top() {
        let items = vec![
            item(ticket(1), InboxKind::Blocked),
            item(InboxRef::Note(1), InboxKind::Note),
            item(ticket(2), InboxKind::Blocked),
        ];
        assert_eq!(keys(&apply_order(items.clone(), &[])), keys(&items));
        let saved = [ticket(9), ticket(2), ticket(1), ticket(2)];
        assert_eq!(
            keys(&apply_order(items, &saved)),
            ["note:1", "ticket:ARC-0002", "ticket:ARC-0001"]
        );
    }

    #[test]
    fn orders_drop_repeats_and_keep_the_first_entries() {
        assert_eq!(
            order(vec![ticket(1), ticket(1), InboxRef::Note(2)]),
            [ticket(1), InboxRef::Note(2)]
        );
        let many = (1..=1001).map(InboxRef::Note).collect();
        let kept = order(many);
        assert_eq!(kept.len(), INBOX_ORDER_MAX);
        assert_eq!(kept.last(), Some(&InboxRef::Note(1000)));
    }

    #[test]
    fn references_parse_by_type() {
        assert_eq!(InboxRef::parse("ticket", "arc-1").unwrap(), ticket(1));
        assert_eq!(
            InboxRef::parse("question", "ARC-Q2").unwrap().to_string(),
            "question:ARC-Q0002"
        );
        assert_eq!(InboxRef::parse("note", "7").unwrap(), InboxRef::Note(7));
        for (item_type, key) in [("note", "0"), ("note", "x"), ("ticket", "7"), ("todo", "1")] {
            assert_eq!(
                InboxRef::parse(item_type, key),
                Err(InboxError::InvalidKey(format!("{item_type}:{key}")))
            );
        }
    }

    #[test]
    fn counts_every_kind() {
        let items = vec![
            item(ticket(1), InboxKind::Merge),
            item(ticket(2), InboxKind::Merge),
            item(InboxRef::Note(1), InboxKind::Note),
        ];
        let counts = InboxCounts::of(&items);
        assert_eq!((counts.merge, counts.note, counts.blocked), (2, 1, 0));
    }

    #[test]
    fn notes_show_sightings_and_the_rest_of_the_body() {
        assert_eq!(
            note_text("Slow login", "Takes\n  5   seconds\r\n", 3, 2),
            (
                String::from("Slow login"),
                Some(String::from("seen 2 times | Takes 5 seconds"))
            )
        );
        assert_eq!(
            note_text("", "First line\nsecond", 3, 1),
            (String::from("First line"), Some(String::from("second")))
        );
        assert_eq!(note_text("Alone", "", 3, 1), (String::from("Alone"), None));
        let (title, _) = note_text(&"x".repeat(130), "", 3, 1);
        assert_eq!(utf16_len(&title), INBOX_TITLE_MAX);
        assert!(title.ends_with("..."));
    }

    #[test]
    fn details_are_clipped_and_blank_ones_dropped() {
        let entry = item(ticket(1), InboxKind::Blocked).with_detail(Some(""));
        assert_eq!(entry.detail, None);
        let entry = item(ticket(1), InboxKind::Blocked).with_detail(Some(&"d".repeat(400)));
        assert_eq!(entry.detail.map(|detail| utf16_len(&detail)), Some(300));
        assert_eq!(clip("short", 10), "short");
    }
}
