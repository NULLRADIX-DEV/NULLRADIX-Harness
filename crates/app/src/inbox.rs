//! The inbox: everything on the boards that waits for the person, in the order they chose.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Board, Id, InboxCounts, InboxError, InboxItem, InboxKind, InboxRef, Priority, ProjectKey,
    QuestionKey, Ticket, TicketKey, TicketStatus, Timestamp, Version, apply_order, compare,
    note_text, order,
};
use uuid::Uuid;

use crate::boards;
use crate::{AppError, Change, ChangeSignal, Clock};

/// A board with the key of its project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxBoard {
    pub id: Id<Board>,
    pub name: String,
    pub project_key: ProjectKey,
}

/// A ticket that waits itself or that an open question is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxTicket {
    pub id: Id<Ticket>,
    pub key: TicketKey,
    pub board: Id<Board>,
    pub title: String,
    pub status: TicketStatus,
    pub priority: Priority,
    pub blocked_reason: Option<String>,
    pub updated_at: Timestamp,
}

/// An open question with the tickets it is about, by ticket number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxQuestion {
    pub key: QuestionKey,
    pub title: String,
    pub occasion: Option<String>,
    pub created_at: Timestamp,
    pub tickets: Vec<Id<Ticket>>,
}

/// An open note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxNote {
    pub number: i64,
    pub board: Id<Board>,
    pub title: String,
    pub body: String,
    pub seen_count: usize,
    pub created_at: Timestamp,
}

/// What the inbox is made of, read in one snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxSnapshot {
    pub boards: Vec<InboxBoard>,
    pub tickets: Vec<InboxTicket>,
    pub questions: Vec<InboxQuestion>,
    pub notes: Vec<InboxNote>,
    /// The saved order; entries that no longer exist are left out.
    pub order: Vec<InboxRef>,
    /// Counts the writes of the saved order.
    pub order_version: Version,
}

/// Id the change feed knows the saved order by.
pub const ORDER_ID: Uuid = Uuid::nil();

/// A written order with the change that announces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderWrite {
    pub order: Vec<InboxRef>,
    pub version: Version,
    pub change: Change,
}

/// The order a client sees after saving, with its new version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedOrder {
    pub order: Vec<InboxRef>,
    pub version: Version,
}

#[async_trait]
pub trait InboxRepository: Send + Sync {
    async fn snapshot(&self) -> Result<InboxSnapshot, AppError>;
    /// Replaces the saved order, dropping gone entries; a stale version is `version-conflict`.
    async fn save_order(
        &self,
        order: &[InboxRef],
        expected: Version,
        at: Timestamp,
    ) -> Result<OrderWrite, AppError>;
    /// Back to the default order; the version must match as for saving.
    async fn clear_order(&self, expected: Version, at: Timestamp) -> Result<OrderWrite, AppError>;
}

/// The inbox with counts of each kind before the kind filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxList {
    pub items: Vec<InboxItem>,
    pub counts: InboxCounts,
    /// Entries of every kind.
    pub total: u32,
    /// Version of the saved order, to send back when saving.
    pub order_version: Version,
}

/// An entry of an order as a client sends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderEntry<'a> {
    pub item_type: &'a str,
    pub key: &'a str,
}

#[must_use]
pub fn invalid(error: &InboxError) -> AppError {
    let code = match error {
        InboxError::InvalidKind(_) => "inbox-kind-invalid",
        InboxError::InvalidKey(_) => "inbox-key-invalid",
    };
    AppError::invalid(code, error.to_string())
}

/// Entries of one board, or of all boards; sorted by urgency.
#[must_use]
pub fn gather(snapshot: &InboxSnapshot, board: Option<Id<Board>>) -> Vec<InboxItem> {
    let boards: HashMap<Id<Board>, &InboxBoard> = snapshot
        .boards
        .iter()
        .map(|found| (found.id, found))
        .collect();
    let tickets: HashMap<Id<Ticket>, &InboxTicket> = snapshot
        .tickets
        .iter()
        .map(|ticket| (ticket.id, ticket))
        .collect();
    let wanted = |at: Option<Id<Board>>| board.is_none() || at == board;
    let place = |mut item: InboxItem, at: Option<Id<Board>>| {
        let found = at.and_then(|id| boards.get(&id));
        item.board = found.map(|found| found.id);
        item.board_name = found.map(|found| found.name.clone());
        item
    };
    let mut covered: HashSet<Id<Ticket>> = HashSet::new();
    let mut items = Vec::new();
    for question in &snapshot.questions {
        let linked: Vec<&InboxTicket> = question
            .tickets
            .iter()
            .filter_map(|id| tickets.get(id).copied())
            .collect();
        let open: Vec<&InboxTicket> = linked
            .iter()
            .copied()
            .filter(|ticket| ticket.status.is_open())
            .collect();
        covered.extend(open.iter().map(|ticket| ticket.id));
        let at = linked.first().map(|ticket| ticket.board);
        if !wanted(at) {
            continue;
        }
        let mut item = InboxItem::new(
            InboxRef::Question(question.key.clone()),
            InboxKind::Question,
            question.title.clone(),
            question.key.project().clone(),
        )
        .with_detail(question.occasion.as_deref());
        item.waiting_tickets = u32::try_from(open.len()).unwrap_or(u32::MAX);
        item.since = question.created_at;
        items.push(place(item, at));
    }
    for ticket in &snapshot.tickets {
        let Some(kind) = InboxKind::of_ticket(ticket.status, ticket.blocked_reason.as_deref())
        else {
            continue;
        };
        if covered.contains(&ticket.id) || !wanted(Some(ticket.board)) {
            continue;
        }
        let detail = (kind != InboxKind::Planning)
            .then_some(ticket.blocked_reason.as_deref())
            .flatten();
        let mut item = InboxItem::new(
            InboxRef::Ticket(ticket.key.clone()),
            kind,
            ticket.title.clone(),
            ticket.key.project().clone(),
        )
        .with_detail(detail);
        item.priority = Some(ticket.priority);
        item.since = ticket.updated_at;
        items.push(place(item, Some(ticket.board)));
    }
    for note in &snapshot.notes {
        let Some(found) = boards.get(&note.board).filter(|_| wanted(Some(note.board))) else {
            continue;
        };
        let (title, detail) = note_text(&note.title, &note.body, note.number, note.seen_count);
        let mut item = InboxItem::new(
            InboxRef::Note(note.number),
            InboxKind::Note,
            title,
            found.project_key.clone(),
        )
        .with_detail(detail.as_deref());
        item.since = note.created_at;
        items.push(place(item, Some(note.board)));
    }
    items.sort_by(compare);
    items
}

pub struct InboxService {
    inbox: Arc<dyn InboxRepository>,
    clock: Arc<dyn Clock>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for InboxService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxService").finish_non_exhaustive()
    }
}

impl InboxService {
    #[must_use]
    pub fn new(
        inbox: Arc<dyn InboxRepository>,
        clock: Arc<dyn Clock>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            inbox,
            clock,
            signal,
        }
    }

    /// The inbox in the saved order; `kinds` such as `merge,accept` keeps only those.
    pub async fn list(
        &self,
        board: Option<Id<Board>>,
        kinds: Option<&str>,
    ) -> Result<InboxList, AppError> {
        let kinds = kinds
            .map(InboxKind::parse_list)
            .transpose()
            .map_err(|error| invalid(&error))?;
        let snapshot = self.inbox.snapshot().await?;
        if let Some(board) = board
            && !snapshot.boards.iter().any(|found| found.id == board)
        {
            return Err(boards::not_found(board));
        }
        let items = apply_order(gather(&snapshot, board), &snapshot.order);
        let counts = InboxCounts::of(&items);
        let total = u32::try_from(items.len()).unwrap_or(u32::MAX);
        let items = match kinds {
            Some(kinds) => items
                .into_iter()
                .filter(|item| kinds.contains(&item.kind))
                .collect(),
            None => items,
        };
        Ok(InboxList {
            items,
            counts,
            total,
            order_version: snapshot.order_version,
        })
    }

    /// Saves the order as read at `expected`; repeats, gone entries and all past 1000 are dropped.
    pub async fn save_order(
        &self,
        entries: &[OrderEntry<'_>],
        expected: Version,
    ) -> Result<SavedOrder, AppError> {
        let parsed = entries
            .iter()
            .map(|entry| InboxRef::parse(entry.item_type, entry.key))
            .collect::<Result<Vec<_>, _>>()
            .map(order)
            .map_err(|error| invalid(&error))?;
        let write = self
            .inbox
            .save_order(&parsed, expected, self.clock.now())
            .await?;
        self.signal.notify(&write.change);
        Ok(SavedOrder {
            order: write.order,
            version: write.version,
        })
    }

    /// Back to the default order; returns the new version of the saved order.
    pub async fn clear_order(&self, expected: Version) -> Result<Version, AppError> {
        let write = self.inbox.clear_order(expected, self.clock.now()).await?;
        self.signal.notify(&write.change);
        Ok(write.version)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use uuid::Uuid;

    use super::*;

    fn project() -> ProjectKey {
        ProjectKey::parse("ARC").unwrap()
    }

    fn board(n: u128) -> InboxBoard {
        InboxBoard {
            id: Id::from_uuid(Uuid::from_u128(n)),
            name: format!("Board {n}"),
            project_key: project(),
        }
    }

    fn ticket(number: u32, board: u128, status: TicketStatus, reason: Option<&str>) -> InboxTicket {
        InboxTicket {
            id: Id::from_uuid(Uuid::from_u128(100 + u128::from(number))),
            key: TicketKey::new(project(), number),
            board: Id::from_uuid(Uuid::from_u128(board)),
            title: format!("Ticket {number}"),
            status,
            priority: Priority::Normal,
            blocked_reason: reason.map(str::to_owned),
            updated_at: Timestamp::from_millis(i64::from(number)),
        }
    }

    fn question(number: u32, tickets: &[u32]) -> InboxQuestion {
        InboxQuestion {
            key: QuestionKey::new(project(), number),
            title: format!("Question {number}"),
            occasion: Some(String::from("while planning")),
            created_at: Timestamp::from_millis(1),
            tickets: tickets
                .iter()
                .map(|number| Id::from_uuid(Uuid::from_u128(100 + u128::from(*number))))
                .collect(),
        }
    }

    fn note(number: i64, board: u128) -> InboxNote {
        InboxNote {
            number,
            board: Id::from_uuid(Uuid::from_u128(board)),
            title: format!("Note {number}"),
            body: String::new(),
            seen_count: 1,
            created_at: Timestamp::from_millis(1),
        }
    }

    fn snapshot() -> InboxSnapshot {
        InboxSnapshot {
            boards: vec![board(1), board(2)],
            tickets: vec![
                ticket(1, 1, TicketStatus::Blocked, Some("Merge: conflict")),
                ticket(2, 1, TicketStatus::Blocked, Some("Human Interaction: look")),
                ticket(3, 2, TicketStatus::InPlanning, Some("stale")),
                ticket(4, 2, TicketStatus::Blocked, Some("waits for the question")),
                ticket(5, 2, TicketStatus::Done, None),
                ticket(
                    6,
                    1,
                    TicketStatus::Blocked,
                    Some("waits for the third question"),
                ),
                ticket(
                    7,
                    2,
                    TicketStatus::Blocked,
                    Some("waits for the third question"),
                ),
            ],
            questions: vec![question(1, &[4, 5]), question(2, &[]), question(3, &[6, 7])],
            notes: vec![note(1, 1)],
            order: Vec::new(),
            order_version: Version::INITIAL,
        }
    }

    fn keys(items: &[InboxItem]) -> Vec<String> {
        items.iter().map(|item| item.item.to_string()).collect()
    }

    #[test]
    fn gathers_waiting_tickets_open_questions_and_notes_by_urgency() {
        let items = gather(&snapshot(), None);
        assert_eq!(
            keys(&items),
            [
                "ticket:ARC-0001",
                "question:ARC-Q0003",
                "question:ARC-Q0001",
                "question:ARC-Q0002",
                "ticket:ARC-0002",
                "ticket:ARC-0003",
                "note:1",
            ]
        );
        let question = &items[2];
        assert_eq!(question.waiting_tickets, 1);
        assert_eq!(question.board_name.as_deref(), Some("Board 2"));
        assert_eq!(question.detail.as_deref(), Some("while planning"));
        assert_eq!(items[3].board, None);
        assert_eq!(items[5].detail, None, "planning shows no reason");
        assert_eq!(items[0].detail.as_deref(), Some("Merge: conflict"));
    }

    #[test]
    fn a_board_keeps_its_entries_and_questions_of_its_first_ticket() {
        let board = |n: u128| Some(Id::from_uuid(Uuid::from_u128(n)));
        assert_eq!(
            keys(&gather(&snapshot(), board(2))),
            ["question:ARC-Q0001", "ticket:ARC-0003"],
            "question 3 asks about a ticket of board 2 too, yet its first ticket is on board 1"
        );
        assert_eq!(
            keys(&gather(&snapshot(), board(1))),
            [
                "ticket:ARC-0001",
                "question:ARC-Q0003",
                "ticket:ARC-0002",
                "note:1"
            ]
        );
    }

    struct Fake {
        snapshot: InboxSnapshot,
        saved: Mutex<(Vec<InboxRef>, Version)>,
    }

    impl Fake {
        fn write(&self, order: Vec<InboxRef>, expected: Version) -> Result<OrderWrite, AppError> {
            let mut saved = self.saved.lock().unwrap();
            if saved.1 != expected {
                return Err(AppError::version_conflict("inbox", "order"));
            }
            let version = expected.next();
            *saved = (order.clone(), version);
            Ok(OrderWrite {
                order,
                version,
                change: Change {
                    seq: version.value(),
                    entity: crate::Entity::InboxOrder,
                    entity_id: ORDER_ID,
                    op: crate::ChangeOp::Update,
                    at: Timestamp::from_millis(0),
                },
            })
        }
    }

    #[async_trait]
    impl InboxRepository for Fake {
        async fn snapshot(&self) -> Result<InboxSnapshot, AppError> {
            let mut snapshot = self.snapshot.clone();
            let saved = self.saved.lock().unwrap();
            snapshot.order.clone_from(&saved.0);
            snapshot.order_version = saved.1;
            Ok(snapshot)
        }

        async fn save_order(
            &self,
            order: &[InboxRef],
            expected: Version,
            _at: Timestamp,
        ) -> Result<OrderWrite, AppError> {
            self.write(order.to_vec(), expected)
        }

        async fn clear_order(
            &self,
            expected: Version,
            _at: Timestamp,
        ) -> Result<OrderWrite, AppError> {
            self.write(Vec::new(), expected)
        }
    }

    fn service() -> (InboxService, ChangeSignal) {
        let signal = ChangeSignal::new();
        let inbox = InboxService::new(
            Arc::new(Fake {
                snapshot: snapshot(),
                saved: Mutex::new((Vec::new(), Version::INITIAL)),
            }),
            Arc::new(crate::SystemClock),
            signal.clone(),
        );
        (inbox, signal)
    }

    fn entry(item_type: &'static str, key: &'static str) -> OrderEntry<'static> {
        OrderEntry { item_type, key }
    }

    #[tokio::test]
    async fn lists_in_the_saved_order_and_filters_kinds_after_counting() {
        let (inbox, _) = service();
        let entries = [
            entry("note", "1"),
            entry("ticket", "arc-1"),
            entry("note", "1"),
        ];
        let saved = inbox.save_order(&entries, Version::INITIAL).await.unwrap();
        assert_eq!(saved.order.len(), 2);
        assert_eq!(saved.version, Version::INITIAL.next());
        let list = inbox.list(None, None).await.unwrap();
        assert_eq!(list.order_version, saved.version);
        assert_eq!(keys(&list.items)[5..], ["note:1", "ticket:ARC-0001"]);
        let list = inbox.list(None, Some("merge, note")).await.unwrap();
        assert_eq!(keys(&list.items), ["note:1", "ticket:ARC-0001"]);
        assert_eq!((list.total, list.counts.question), (7, 3));
        let cleared = inbox.clear_order(saved.version).await.unwrap();
        assert_eq!(cleared, saved.version.next());
        let list = inbox.list(None, None).await.unwrap();
        assert_eq!(keys(&list.items)[0], "ticket:ARC-0001");
        assert_eq!(list.order_version, cleared);
    }

    #[tokio::test]
    async fn every_write_of_the_order_notifies_and_a_stale_version_is_refused() {
        let (inbox, signal) = service();
        let mut head = signal.subscribe();
        let saved = inbox
            .save_order(&[entry("note", "1")], Version::INITIAL)
            .await
            .unwrap();
        assert!(head.has_changed().unwrap());
        assert_eq!(*head.borrow_and_update(), 2);

        let stale = inbox
            .save_order(&[entry("note", "1")], Version::INITIAL)
            .await
            .unwrap_err();
        assert_eq!(stale.code(), "version-conflict");
        let stale = inbox.clear_order(Version::INITIAL).await.unwrap_err();
        assert_eq!(stale.code(), "version-conflict");
        assert!(
            !head.has_changed().unwrap(),
            "refused writes announce nothing"
        );

        inbox.clear_order(saved.version).await.unwrap();
        assert!(head.has_changed().unwrap());
        assert_eq!(*head.borrow_and_update(), 3);
    }

    #[tokio::test]
    async fn refuses_unknown_kinds_boards_and_keys() {
        let (inbox, _) = service();
        let error = inbox.list(None, Some("nonsense")).await.unwrap_err();
        assert_eq!(error.code(), "inbox-kind-invalid");
        let missing = Id::from_uuid(Uuid::from_u128(9));
        let error = inbox.list(Some(missing), None).await.unwrap_err();
        assert_eq!(error.code(), "board-not-found");
        let error = inbox
            .save_order(&[entry("todo", "1")], Version::INITIAL)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "inbox-key-invalid");
    }

    #[tokio::test]
    async fn keeps_the_first_thousand_entries_of_a_longer_order() {
        let (inbox, _) = service();
        let many: Vec<String> = (1..=1001).map(|n: i64| n.to_string()).collect();
        let entries: Vec<OrderEntry<'_>> = many
            .iter()
            .map(|key| OrderEntry {
                item_type: "note",
                key,
            })
            .collect();
        let saved = inbox.save_order(&entries, Version::INITIAL).await.unwrap();
        assert_eq!(saved.order.len(), 1000);
        assert_eq!(saved.order.last(), Some(&InboxRef::Note(1000)));
    }
}
