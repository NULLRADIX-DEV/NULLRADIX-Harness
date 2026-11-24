use async_trait::async_trait;
use nullradix_app::{AppError, Change, ChangeFeed, ChangeOp, Entity};
use nullradix_domain::Timestamp;
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::{Store, StoreError};

/// Appends a change inside the caller's transaction.
pub(crate) async fn record(
    connection: &mut SqliteConnection,
    entity: Entity,
    entity_id: Uuid,
    op: ChangeOp,
    at: Timestamp,
) -> Result<Change, StoreError> {
    let entity_name = entity.as_str();
    let op_name = op.as_str();
    let millis = at.as_millis();
    let seq = sqlx::query_scalar!(
        "INSERT INTO change_feed (entity, entity_id, op, at) VALUES (?, ?, ?, ?) RETURNING seq",
        entity_name,
        entity_id,
        op_name,
        millis,
    )
    .fetch_one(connection)
    .await?;
    Ok(Change {
        seq,
        entity,
        entity_id,
        op,
        at,
    })
}

#[async_trait]
impl ChangeFeed for Store {
    async fn changes_since(&self, seq: i64, limit: u32) -> Result<Vec<Change>, AppError> {
        let rows = sqlx::query!(
            r#"SELECT seq AS "seq!", entity, entity_id AS "entity_id: Uuid", op, at
               FROM change_feed WHERE seq > ? ORDER BY seq LIMIT ?"#,
            seq,
            limit,
        )
        .fetch_all(&self.reader)
        .await
        .map_err(StoreError::from)?;
        let changes = rows
            .into_iter()
            .map(|row| {
                let entity = Entity::parse(&row.entity).ok_or_else(|| {
                    StoreError::Corrupt(format!("unknown entity `{}`", row.entity))
                })?;
                let op = ChangeOp::parse(&row.op).ok_or_else(|| {
                    StoreError::Corrupt(format!("unknown change op `{}`", row.op))
                })?;
                Ok(Change {
                    seq: row.seq,
                    entity,
                    entity_id: row.entity_id,
                    op,
                    at: Timestamp::from_millis(row.at),
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_changes_after_a_sequence_number_in_order() {
        let store = Store::in_memory().await.unwrap();
        let mut tx = store.begin().await.unwrap();
        let at = Timestamp::from_millis(1_000);
        let first = record(
            &mut tx,
            Entity::Project,
            Uuid::from_u128(1),
            ChangeOp::Insert,
            at,
        )
        .await
        .unwrap();
        let second = record(
            &mut tx,
            Entity::Project,
            Uuid::from_u128(2),
            ChangeOp::Update,
            at,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(
            store.changes_since(first.seq, 10).await.unwrap(),
            vec![second]
        );
        assert_eq!(store.changes_since(0, 10).await.unwrap().len(), 2);
        assert_eq!(store.changes_since(0, 1).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn rolled_back_changes_are_not_visible() {
        let store = Store::in_memory().await.unwrap();
        let mut tx = store.begin().await.unwrap();
        record(
            &mut tx,
            Entity::Project,
            Uuid::from_u128(1),
            ChangeOp::Delete,
            Timestamp::from_millis(1),
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(store.changes_since(0, 10).await.unwrap(), Vec::new());
    }
}
