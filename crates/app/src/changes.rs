use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::Timestamp;
use strum::{EnumString, IntoStaticStr};
use tokio::sync::watch;
use uuid::Uuid;

use crate::AppError;

/// Kind of entity a change refers to; the wire name is kebab-case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum Entity {
    Project,
    Board,
    Ticket,
    Run,
    Runner,
    Repository,
    Worktree,
    Comment,
    Note,
    Question,
    Session,
    InboxOrder,
    Favorite,
    Account,
    Theme,
    Setting,
    Customization,
}

impl Entity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum ChangeOp {
    Insert,
    Update,
    Delete,
}

impl ChangeOp {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// One committed change; `seq` orders all changes and is the resume point of event streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub seq: i64,
    pub entity: Entity,
    pub entity_id: Uuid,
    pub op: ChangeOp,
    pub at: Timestamp,
}

/// Wakes event streams after a commit; the newest `seq` is all it carries.
#[derive(Debug, Clone)]
pub struct ChangeSignal {
    head: Arc<watch::Sender<i64>>,
}

impl Default for ChangeSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl ChangeSignal {
    #[must_use]
    pub fn new() -> Self {
        Self {
            head: Arc::new(watch::Sender::new(0)),
        }
    }

    pub fn notify(&self, change: &Change) {
        self.head.send_if_modified(|head| {
            let newer = change.seq > *head;
            if newer {
                *head = change.seq;
            }
            newer
        });
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<i64> {
        self.head.subscribe()
    }
}

#[async_trait]
pub trait ChangeFeed: Send + Sync {
    /// Changes committed after `seq`, oldest first, at most `limit`.
    async fn changes_since(&self, seq: i64, limit: u32) -> Result<Vec<Change>, AppError>;
    /// Sequence number of the newest change, or 0 when there is none.
    async fn head(&self) -> Result<i64, AppError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(seq: i64) -> Change {
        Change {
            seq,
            entity: Entity::Project,
            entity_id: Uuid::from_u128(1),
            op: ChangeOp::Insert,
            at: Timestamp::from_millis(0),
        }
    }

    #[test]
    fn the_signal_only_moves_forward() {
        let signal = ChangeSignal::new();
        let mut head = signal.subscribe();
        signal.notify(&change(5));
        assert!(head.has_changed().unwrap());
        assert_eq!(*head.borrow_and_update(), 5);
        signal.notify(&change(3));
        assert!(!head.has_changed().unwrap());
        assert_eq!(*head.borrow(), 5);
    }

    #[test]
    fn wire_names_round_trip() {
        assert_eq!(Entity::Project.as_str(), "project");
        assert_eq!("project".parse::<Entity>(), Ok(Entity::Project));
        assert_eq!(Entity::InboxOrder.as_str(), "inbox-order");
        assert_eq!(Entity::Setting.as_str(), "setting");
        assert_eq!("inbox-order".parse::<Entity>(), Ok(Entity::InboxOrder));
        assert_eq!(Entity::Account.as_str(), "account");
        assert!("planet".parse::<Entity>().is_err());
        for op in [ChangeOp::Insert, ChangeOp::Update, ChangeOp::Delete] {
            assert_eq!(op.as_str().parse::<ChangeOp>(), Ok(op));
        }
        assert!("upsert".parse::<ChangeOp>().is_err());
    }
}
