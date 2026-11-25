//! The inbox: everything on the boards that waits for the person, in the order they chose.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Board, Id, InboxCounts, InboxError, InboxItem, InboxKind, InboxRef, Priority, ProjectKey,
    QuestionKey, Ticket, TicketKey, TicketStatus, Timestamp, apply_order, compare, note_text,
    order,
};

use crate::AppError;
use crate::boards;

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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InboxSnapshot {
    pub boards: Vec<InboxBoard>,
    pub tickets: Vec<InboxTicket>,
    pub questions: Vec<InboxQuestion>,
    pub notes: Vec<InboxNote>,
    /// The saved order; entries that no longer exist are left out.
    pub order: Vec<InboxRef>,
}

#[async_trait]
pub trait InboxRepository: Send + Sync {
    async fn snapshot(&self) -> Result<InboxSnapshot, AppError>;
    /// Replaces the saved order; entries that do not exist are dropped.
    async fn save_order(&self, order: &[InboxRef]) -> Result<Vec<InboxRef>, AppError>;
    async fn clear_order(&self) -> Result<(), AppError>;
}

/// The inbox with counts of each kind before the kind filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxList {
    pub items: Vec<InboxItem>,
    pub counts: InboxCounts,
    /// Entries of every kind.
    pub total: u32,
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
        InboxError::TooMany(_) => "inbox-order-too-long",
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
}

impl std::fmt::Debug for InboxService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxService").finish_non_exhaustive()
    }
}

impl InboxService {
    #[must_use]
    pub fn new(inbox: Arc<dyn InboxRepository>) -> Self {
        Self { inbox }
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
        })
    }

    /// Saves the order of entries; repeats and entries that do not exist are dropped.
    pub async fn save_order(&self, entries: &[OrderEntry<'_>]) -> Result<Vec<InboxRef>, AppError> {
        let parsed = entries
            .iter()
            .map(|entry| InboxRef::parse(entry.item_type, entry.key))
            .collect::<Result<Vec<_>, _>>()
            .and_then(order)
            .map_err(|error| invalid(&error))?;
        self.inbox.save_order(&parsed).await
    }

    /// Back to the default order.
    pub async fn clear_order(&self) -> Result<(), AppError> {
        self.inbox.clear_order().await
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
            ],
            questions: vec![question(1, &[4, 5]), question(2, &[])],
            notes: vec![note(1, 1)],
            order: Vec::new(),
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
                "question:ARC-Q0001",
                "question:ARC-Q0002",
                "ticket:ARC-0002",
                "ticket:ARC-0003",
                "note:1",
            ]
        );
        let question = &items[1];
        assert_eq!(question.waiting_tickets, 1);
        assert_eq!(question.board_name.as_deref(), Some("Board 2"));
        assert_eq!(question.detail.as_deref(), Some("while planning"));
        assert_eq!(items[2].board, None);
        assert_eq!(items[4].detail, None, "planning shows no reason");
        assert_eq!(items[0].detail.as_deref(), Some("Merge: conflict"));
    }

    #[test]
    fn a_board_keeps_its_entries_and_questions_of_its_first_ticket() {
        let board = Some(Id::from_uuid(Uuid::from_u128(2)));
        assert_eq!(
            keys(&gather(&snapshot(), board)),
            ["question:ARC-Q0001", "ticket:ARC-0003"]
        );
    }

    struct Fake {
        snapshot: InboxSnapshot,
        saved: Mutex<Vec<InboxRef>>,
    }

    #[async_trait]
    impl InboxRepository for Fake {
        async fn snapshot(&self) -> Result<InboxSnapshot, AppError> {
            let mut snapshot = self.snapshot.clone();
            snapshot.order = self.saved.lock().unwrap().clone();
            Ok(snapshot)
        }

        async fn save_order(&self, order: &[InboxRef]) -> Result<Vec<InboxRef>, AppError> {
            order.clone_into(&mut self.saved.lock().unwrap());
            Ok(order.to_vec())
        }

        async fn clear_order(&self) -> Result<(), AppError> {
            self.saved.lock().unwrap().clear();
            Ok(())
        }
    }

    fn service() -> InboxService {
        InboxService::new(Arc::new(Fake {
            snapshot: snapshot(),
            saved: Mutex::new(Vec::new()),
        }))
    }

    #[tokio::test]
    async fn lists_in_the_saved_order_and_filters_kinds_after_counting() {
        let inbox = service();
        let entries = [
            OrderEntry {
                item_type: "note",
                key: "1",
            },
            OrderEntry {
                item_type: "ticket",
                key: "arc-1",
            },
            OrderEntry {
                item_type: "note",
                key: "1",
            },
        ];
        let saved = inbox.save_order(&entries).await.unwrap();
        assert_eq!(saved.len(), 2);
        let list = inbox.list(None, None).await.unwrap();
        assert_eq!(keys(&list.items)[4..], ["note:1", "ticket:ARC-0001"]);
        let list = inbox.list(None, Some("merge, note")).await.unwrap();
        assert_eq!(keys(&list.items), ["note:1", "ticket:ARC-0001"]);
        assert_eq!((list.total, list.counts.question), (6, 2));
        inbox.clear_order().await.unwrap();
        let list = inbox.list(None, None).await.unwrap();
        assert_eq!(keys(&list.items)[0], "ticket:ARC-0001");
    }

    #[tokio::test]
    async fn refuses_unknown_kinds_boards_and_keys() {
        let inbox = service();
        let error = inbox.list(None, Some("nonsense")).await.unwrap_err();
        assert_eq!(error.code(), "inbox-kind-invalid");
        let missing = Id::from_uuid(Uuid::from_u128(9));
        let error = inbox.list(Some(missing), None).await.unwrap_err();
        assert_eq!(error.code(), "board-not-found");
        let entry = OrderEntry {
            item_type: "todo",
            key: "1",
        };
        let error = inbox.save_order(&[entry]).await.unwrap_err();
        assert_eq!(error.code(), "inbox-key-invalid");
        let many: Vec<String> = (1..=1001).map(|n: i64| n.to_string()).collect();
        let entries: Vec<OrderEntry<'_>> = many
            .iter()
            .map(|key| OrderEntry {
                item_type: "note",
                key,
            })
            .collect();
        let error = inbox.save_order(&entries).await.unwrap_err();
        assert_eq!(error.code(), "inbox-order-too-long");
    }
}
