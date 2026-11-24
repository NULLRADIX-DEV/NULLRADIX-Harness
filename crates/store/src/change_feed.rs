use nullradix_domain::Timestamp;
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::{Store, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeOp {
    Insert,
    Update,
    Delete,
}

impl ChangeOp {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "insert" => Ok(Self::Insert),
            "update" => Ok(Self::Update),
            "delete" => Ok(Self::Delete),
            other => Err(StoreError::Corrupt(format!("unknown change op `{other}`"))),
        }
    }
}

/// One committed change; `seq` is the resume point for event streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub seq: i64,
    pub entity: String,
    pub entity_id: Uuid,
    pub op: ChangeOp,
    pub at: Timestamp,
}

/// Appends a change inside the caller's transaction.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "repositories record changes once they exist")
)]
pub(crate) async fn record(
    connection: &mut SqliteConnection,
    entity: &str,
    entity_id: Uuid,
    op: ChangeOp,
    at: Timestamp,
) -> Result<i64, StoreError> {
    let op = op.as_str();
    let at = at.as_millis();
    let seq = sqlx::query_scalar!(
        "INSERT INTO change_feed (entity, entity_id, op, at) VALUES (?, ?, ?, ?) RETURNING seq",
        entity,
        entity_id,
        op,
        at,
    )
    .fetch_one(connection)
    .await?;
    Ok(seq)
}

impl Store {
    /// Changes committed after `seq`, oldest first.
    pub async fn changes_since(&self, seq: i64, limit: u32) -> Result<Vec<Change>, StoreError> {
        let rows = sqlx::query!(
            r#"SELECT seq AS "seq!", entity, entity_id AS "entity_id: Uuid", op, at
               FROM change_feed WHERE seq > ? ORDER BY seq LIMIT ?"#,
            seq,
            limit,
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(Change {
                    seq: row.seq,
                    entity: row.entity,
                    entity_id: row.entity_id,
                    op: ChangeOp::parse(&row.op)?,
                    at: Timestamp::from_millis(row.at),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_changes_after_a_sequence_number_in_order() {
        let store = Store::in_memory().await.unwrap();
        let mut tx = store.begin().await.unwrap();
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let at = Timestamp::from_millis(1_000);
        let seq = record(&mut tx, "project", first, ChangeOp::Insert, at)
            .await
            .unwrap();
        record(&mut tx, "project", second, ChangeOp::Update, at)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let changes = store.changes_since(seq, 10).await.unwrap();
        assert_eq!(
            changes,
            vec![Change {
                seq: seq + 1,
                entity: String::from("project"),
                entity_id: second,
                op: ChangeOp::Update,
                at,
            }]
        );
        assert_eq!(store.changes_since(0, 10).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn rolled_back_changes_are_not_visible() {
        let store = Store::in_memory().await.unwrap();
        let mut tx = store.begin().await.unwrap();
        record(
            &mut tx,
            "project",
            Uuid::from_u128(1),
            ChangeOp::Delete,
            Timestamp::from_millis(1),
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(store.changes_since(0, 10).await.unwrap(), Vec::new());
    }

    #[test]
    fn parses_every_op_it_writes() {
        for op in [ChangeOp::Insert, ChangeOp::Update, ChangeOp::Delete] {
            assert_eq!(ChangeOp::parse(op.as_str()).unwrap(), op);
        }
        assert!(ChangeOp::parse("upsert").is_err());
    }
}
