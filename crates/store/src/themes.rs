use async_trait::async_trait;
use nullradix_app::theme_package::{manifest_json, parse_manifest};
use nullradix_app::themes::{SELECTION_FEED_ID, ThemeRepository, ThemeWrite};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{InstalledTheme, InstalledThemeRecord, ThemeId, Timestamp, Version};
use sqlx::{Sqlite, SqliteConnection, Transaction};
use uuid::Uuid;

use crate::error::{database, missed_guard};
use crate::{Store, StoreError, change_feed};

/// Key of the chosen theme in the settings table.
const SELECTION_KEY: &str = "theme";

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid theme {what}"))
}

struct Row {
    manifest: String,
    installed_at: i64,
    updated_at: i64,
    version: i64,
}

impl Row {
    fn into_theme(self) -> Result<InstalledTheme, StoreError> {
        let manifest = parse_manifest(&self.manifest).map_err(|_| corrupt("manifest"))?;
        Ok(InstalledTheme::restore(InstalledThemeRecord {
            manifest,
            installed_at: Timestamp::from_millis(self.installed_at),
            updated_at: Timestamp::from_millis(self.updated_at),
            version: Version::from_stored(self.version),
        }))
    }
}

async fn load(
    connection: &mut SqliteConnection,
    only: Option<&str>,
) -> Result<Vec<InstalledTheme>, StoreError> {
    let rows = sqlx::query_as!(
        Row,
        "SELECT manifest, installed_at, updated_at, version
         FROM installed_themes WHERE (?1 IS NULL OR theme_id = ?1) ORDER BY theme_id",
        only,
    )
    .fetch_all(&mut *connection)
    .await?;
    rows.into_iter().map(Row::into_theme).collect()
}

async fn selection(connection: &mut SqliteConnection) -> Result<Option<String>, StoreError> {
    let value = sqlx::query_scalar!("SELECT value FROM settings WHERE key = ?", SELECTION_KEY)
        .fetch_optional(&mut *connection)
        .await?;
    value
        .map(|text| serde_json::from_str::<String>(&text).map_err(|_| corrupt("selection")))
        .transpose()
}

struct Columns {
    id: String,
    name: String,
    version: String,
    author: String,
    sdk_version: String,
    scheme: &'static str,
    claude: i64,
    manifest: String,
}

fn columns(theme: &InstalledTheme) -> Columns {
    let manifest = &theme.record().manifest;
    let record = manifest.record();
    Columns {
        id: record.id.to_string(),
        name: record.name.clone(),
        version: record.version.clone(),
        author: record.author.clone(),
        sdk_version: record.sdk_version.clone(),
        scheme: record.color_scheme.as_str(),
        claude: i64::from(record.claude_theme),
        manifest: manifest_json(manifest).to_string(),
    }
}

pub(crate) struct Write(Transaction<'static, Sqlite>);

#[async_trait]
impl ThemeWrite for Write {
    async fn theme(&mut self, id: &ThemeId) -> Result<Option<InstalledTheme>, AppError> {
        Ok(load(&mut self.0, Some(id.as_str())).await?.pop())
    }

    async fn insert(&mut self, theme: &InstalledTheme, feed_id: Uuid) -> Result<Change, AppError> {
        let record = theme.record();
        let c = columns(theme);
        let installed_at = record.installed_at.as_millis();
        let updated_at = record.updated_at.as_millis();
        let version = record.version.value();
        sqlx::query!(
            "INSERT INTO installed_themes (theme_id, feed_id, name, theme_version, author,
                 sdk_version, color_scheme, has_claude_theme, manifest, installed_at, updated_at,
                 version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            c.id,
            feed_id,
            c.name,
            c.version,
            c.author,
            c.sdk_version,
            c.scheme,
            c.claude,
            c.manifest,
            installed_at,
            updated_at,
            version,
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(change_feed::record(
            &mut self.0,
            Entity::Theme,
            feed_id,
            ChangeOp::Insert,
            record.installed_at,
        )
        .await?)
    }

    async fn update(
        &mut self,
        theme: &InstalledTheme,
        previous: Version,
    ) -> Result<Change, AppError> {
        let record = theme.record();
        let c = columns(theme);
        let updated_at = record.updated_at.as_millis();
        let version = record.version.value();
        let previous = previous.value();
        let updated = sqlx::query_scalar!(
            r#"UPDATE installed_themes
               SET name = ?, theme_version = ?, author = ?, sdk_version = ?, color_scheme = ?,
                   has_claude_theme = ?, manifest = ?, updated_at = ?, version = ?
               WHERE theme_id = ? AND version = ?
               RETURNING feed_id AS "feed_id: Uuid""#,
            c.name,
            c.version,
            c.author,
            c.sdk_version,
            c.scheme,
            c.claude,
            c.manifest,
            updated_at,
            version,
            c.id,
            previous,
        )
        .fetch_optional(&mut *self.0)
        .await
        .map_err(database)?;
        let Some(feed_id) = updated else {
            let exists = sqlx::query_scalar!(
                "SELECT version FROM installed_themes WHERE theme_id = ?",
                c.id
            )
            .fetch_optional(&mut *self.0)
            .await;
            return Err(missed_guard(
                exists,
                AppError::version_conflict("theme", &c.id),
                nullradix_app::themes::theme_not_found(&c.id),
            ));
        };
        Ok(change_feed::record(
            &mut self.0,
            Entity::Theme,
            feed_id,
            ChangeOp::Update,
            record.updated_at,
        )
        .await?)
    }

    async fn delete(
        &mut self,
        id: &ThemeId,
        expected: Version,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let name = id.as_str();
        let stored = sqlx::query_scalar!(
            "SELECT version FROM installed_themes WHERE theme_id = ?",
            name
        )
        .fetch_optional(&mut *self.0)
        .await
        .map_err(database)?;
        let Some(stored) = stored else {
            return Ok(None);
        };
        if stored != expected.value() {
            return Err(AppError::version_conflict("theme", id));
        }
        let gone = sqlx::query_scalar!(
            r#"DELETE FROM installed_themes WHERE theme_id = ? RETURNING feed_id AS "feed_id: Uuid""#,
            name
        )
        .fetch_one(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(Some(
            change_feed::record(&mut self.0, Entity::Theme, gone, ChangeOp::Delete, at).await?,
        ))
    }

    async fn selection(&mut self) -> Result<Option<String>, AppError> {
        Ok(selection(&mut self.0).await?)
    }

    async fn select(&mut self, id: &str, at: Timestamp) -> Result<Option<Change>, AppError> {
        if selection(&mut self.0).await?.as_deref() == Some(id) {
            return Ok(None);
        }
        let value = serde_json::to_string(id).map_err(|_| corrupt("selection"))?;
        let millis = at.as_millis();
        sqlx::query!(
            "INSERT INTO settings (key, value, version, updated_at) VALUES (?, ?, 1, ?)
             ON CONFLICT (key) DO UPDATE
                 SET value = excluded.value, version = version + 1,
                     updated_at = excluded.updated_at",
            SELECTION_KEY,
            value,
            millis,
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(Some(
            change_feed::record(
                &mut self.0,
                Entity::Setting,
                SELECTION_FEED_ID,
                ChangeOp::Update,
                at,
            )
            .await?,
        ))
    }

    async fn commit(self: Box<Self>) -> Result<(), AppError> {
        self.0.commit().await.map_err(database)
    }
}

#[async_trait]
impl ThemeRepository for Store {
    async fn begin(&self) -> Result<Box<dyn ThemeWrite>, AppError> {
        Ok(Box::new(Write(self.begin().await?)))
    }

    async fn list(&self) -> Result<Vec<InstalledTheme>, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        Ok(load(&mut read, None).await?)
    }

    async fn get(&self, id: &ThemeId) -> Result<Option<InstalledTheme>, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        Ok(load(&mut read, Some(id.as_str())).await?.pop())
    }

    async fn selection(&self) -> Result<Option<String>, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        Ok(selection(&mut read).await?)
    }
}
