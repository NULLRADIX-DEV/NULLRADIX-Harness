use async_trait::async_trait;
use nullradix_app::customizations::{CustomizationRepository, CustomizationWrite, exists, missing};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    Base, ExtensionPoint, Overlay, OverlayKind, OverlayName, OverlayRecord, Timestamp, Version,
};
use sqlx::{Sqlite, SqliteConnection, Transaction};
use uuid::Uuid;

use crate::error::{database, missed_guard, unique_violation};
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid customization {what}"))
}

struct Row {
    point: String,
    name: String,
    kind: String,
    content: String,
    base_content: Option<String>,
    base_hash: Option<String>,
    enabled: i64,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

impl Row {
    fn into_overlay(self) -> Result<Overlay, StoreError> {
        let base = match (self.base_content, self.base_hash) {
            (Some(content), Some(hash)) => Some(Base { content, hash }),
            (None, None) => None,
            _ => return Err(corrupt("base")),
        };
        Ok(Overlay::restore(OverlayRecord {
            point: self
                .point
                .parse::<ExtensionPoint>()
                .map_err(|_| corrupt("point"))?,
            name: OverlayName::parse(&self.name).map_err(|_| corrupt("name"))?,
            kind: self
                .kind
                .parse::<OverlayKind>()
                .map_err(|_| corrupt("kind"))?,
            content: self.content,
            base,
            enabled: self.enabled != 0,
            created_at: Timestamp::from_millis(self.created_at),
            updated_at: Timestamp::from_millis(self.updated_at),
            version: Version::from_stored(self.version),
        }))
    }
}

async fn load(
    connection: &mut SqliteConnection,
    only: Option<(&str, &str)>,
) -> Result<Vec<Overlay>, StoreError> {
    let (point, name) = only.unzip();
    let rows = sqlx::query_as!(
        Row,
        "SELECT point, name, kind, content, base_content, base_hash, enabled, created_at,
                updated_at, version
         FROM customization_overlays
         WHERE (?1 IS NULL OR point = ?1) AND (?2 IS NULL OR name = ?2)
         ORDER BY CASE point WHEN 'skill' THEN 0 ELSE 1 END, name",
        point,
        name,
    )
    .fetch_all(&mut *connection)
    .await?;
    rows.into_iter().map(Row::into_overlay).collect()
}

pub(crate) struct Write(Transaction<'static, Sqlite>);

#[async_trait]
impl CustomizationWrite for Write {
    async fn overlay(
        &mut self,
        point: ExtensionPoint,
        name: &OverlayName,
    ) -> Result<Option<Overlay>, AppError> {
        Ok(load(&mut self.0, Some((point.as_str(), name.as_str())))
            .await?
            .pop())
    }

    async fn insert(&mut self, overlay: &Overlay, feed_id: Uuid) -> Result<Change, AppError> {
        let record = overlay.record();
        let (point, kind) = (record.point.as_str(), record.kind.as_str());
        let name = record.name.as_str();
        let base_content = record.base.as_ref().map(|base| base.content.as_str());
        let base_hash = record.base.as_ref().map(|base| base.hash.as_str());
        let created_at = record.created_at.as_millis();
        let updated_at = record.updated_at.as_millis();
        let version = record.version.value();
        let result = sqlx::query!(
            "INSERT INTO customization_overlays (id, point, name, kind, content, base_content,
                 base_hash, enabled, created_at, updated_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            feed_id,
            point,
            name,
            kind,
            record.content,
            base_content,
            base_hash,
            record.enabled,
            created_at,
            updated_at,
            version,
        )
        .execute(&mut *self.0)
        .await;
        if let Err(error) = result {
            return Err(
                if unique_violation(&error, "customization_overlays.point") {
                    exists(record.point, &record.name)
                } else {
                    database(error)
                },
            );
        }
        Ok(change_feed::record(
            &mut self.0,
            Entity::Customization,
            feed_id,
            ChangeOp::Insert,
            record.created_at,
        )
        .await?)
    }

    async fn update(&mut self, overlay: &Overlay, previous: Version) -> Result<Change, AppError> {
        let record = overlay.record();
        let (point, name) = (record.point.as_str(), record.name.as_str());
        let base_content = record.base.as_ref().map(|base| base.content.as_str());
        let base_hash = record.base.as_ref().map(|base| base.hash.as_str());
        let updated_at = record.updated_at.as_millis();
        let version = record.version.value();
        let previous = previous.value();
        let updated = sqlx::query_scalar!(
            r#"UPDATE customization_overlays
               SET content = ?, base_content = ?, base_hash = ?, enabled = ?, updated_at = ?,
                   version = ?
               WHERE point = ? AND name = ? AND version = ?
               RETURNING id AS "id: Uuid""#,
            record.content,
            base_content,
            base_hash,
            record.enabled,
            updated_at,
            version,
            point,
            name,
            previous,
        )
        .fetch_optional(&mut *self.0)
        .await
        .map_err(database)?;
        let Some(id) = updated else {
            let present = sqlx::query_scalar!(
                "SELECT version FROM customization_overlays WHERE point = ? AND name = ?",
                point,
                name
            )
            .fetch_optional(&mut *self.0)
            .await;
            return Err(missed_guard(
                present,
                AppError::version_conflict("customization", format!("{point} {name}")),
                missing(record.point, &record.name),
            ));
        };
        Ok(change_feed::record(
            &mut self.0,
            Entity::Customization,
            id,
            ChangeOp::Update,
            record.updated_at,
        )
        .await?)
    }

    async fn delete(
        &mut self,
        point: ExtensionPoint,
        name: &OverlayName,
        expected: Version,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let (point_name, name_text) = (point.as_str(), name.as_str());
        let stored = sqlx::query_scalar!(
            "SELECT version FROM customization_overlays WHERE point = ? AND name = ?",
            point_name,
            name_text
        )
        .fetch_optional(&mut *self.0)
        .await
        .map_err(database)?;
        let Some(stored) = stored else {
            return Ok(None);
        };
        if stored != expected.value() {
            return Err(AppError::version_conflict(
                "customization",
                format!("{point_name} {name_text}"),
            ));
        }
        let id = sqlx::query_scalar!(
            r#"DELETE FROM customization_overlays WHERE point = ? AND name = ?
               RETURNING id AS "id: Uuid""#,
            point_name,
            name_text
        )
        .fetch_one(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(Some(
            change_feed::record(&mut self.0, Entity::Customization, id, ChangeOp::Delete, at)
                .await?,
        ))
    }

    async fn commit(self: Box<Self>) -> Result<(), AppError> {
        self.0.commit().await.map_err(database)
    }
}

#[async_trait]
impl CustomizationRepository for Store {
    async fn begin(&self) -> Result<Box<dyn CustomizationWrite>, AppError> {
        Ok(Box::new(Write(self.begin().await?)))
    }

    async fn list(&self) -> Result<Vec<Overlay>, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        Ok(load(&mut read, None).await?)
    }

    async fn get(
        &self,
        point: ExtensionPoint,
        name: &OverlayName,
    ) -> Result<Option<Overlay>, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        Ok(load(&mut read, Some((point.as_str(), name.as_str())))
            .await?
            .pop())
    }
}
