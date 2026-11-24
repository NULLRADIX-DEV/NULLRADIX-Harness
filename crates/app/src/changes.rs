use async_trait::async_trait;
use nullradix_domain::Timestamp;
use strum::{EnumString, IntoStaticStr};
use uuid::Uuid;

use crate::AppError;

/// Kind of entity a change refers to; the wire name is kebab-case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum Entity {
    Project,
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
        assert_eq!(Entity::Project.as_str(), "project");
        assert_eq!("project".parse::<Entity>(), Ok(Entity::Project));
        assert!("planet".parse::<Entity>().is_err());
        for op in [ChangeOp::Insert, ChangeOp::Update, ChangeOp::Delete] {
            assert_eq!(op.as_str().parse::<ChangeOp>(), Ok(op));
        }
        assert!("upsert".parse::<ChangeOp>().is_err());
    }
}
