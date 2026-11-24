use async_trait::async_trait;
use nullradix_domain::Timestamp;
use uuid::Uuid;

use crate::AppError;

/// Kind of entity a change refers to; the wire name is kebab-case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Entity {
    Project,
}

impl Entity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "project" => Some(Self::Project),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeOp {
    Insert,
    Update,
    Delete,
}

impl ChangeOp {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "insert" => Some(Self::Insert),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            _ => None,
        }
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

#[async_trait]
pub trait ChangeFeed: Send + Sync {
    /// Changes committed after `seq`, oldest first, at most `limit`.
    async fn changes_since(&self, seq: i64, limit: u32) -> Result<Vec<Change>, AppError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_round_trip() {
        assert_eq!(
            Entity::parse(Entity::Project.as_str()),
            Some(Entity::Project)
        );
        assert_eq!(Entity::parse("planet"), None);
        for op in [ChangeOp::Insert, ChangeOp::Update, ChangeOp::Delete] {
            assert_eq!(ChangeOp::parse(op.as_str()), Some(op));
        }
        assert_eq!(ChangeOp::parse("upsert"), None);
    }
}
