use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use nullradix_app::theme_package::{manifest_json, parse_manifest};
use nullradix_app::theme_settings::{parse_settings, settings_json};
use nullradix_app::themes::{
    SELECTION_FEED_ID, StoredSettings, ThemeRepository, ThemeWrite, theme_not_found,
};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    InstalledTheme, InstalledThemeRecord, SourceId, ThemeId, ThemeSettingValues, Timestamp, Version,
};
use sqlx::{Sqlite, SqliteConnection, Transaction};
use uuid::Uuid;

use crate::error::{database, missed_guard};
use crate::{Store, StoreError, change_feed};

/// Key of the chosen theme in the settings table.
const SELECTION_KEY: &str = "theme";

const ORIGIN_PREFIX: &str = "theme-origin.";

const CODE_PREFIX: &str = "theme-code.";

/// Settings key of the values chosen for one theme.
fn settings_key(id: &ThemeId) -> String {
    format!("theme-settings.{id}")
}

/// Settings key of the catalog source a theme was installed from.
fn origin_key(id: &ThemeId) -> String {
    format!("{ORIGIN_PREFIX}{id}")
}

/// Settings key of the code switch of a theme; the row exists only while the switch is on.
fn code_key(id: &ThemeId) -> String {
    format!("{CODE_PREFIX}{id}")
}

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

async fn stored_settings(
    connection: &mut SqliteConnection,
    id: &ThemeId,
) -> Result<Option<StoredSettings>, StoreError> {
    let key = settings_key(id);
    let row = sqlx::query!("SELECT value, version FROM settings WHERE key = ?", key)
        .fetch_optional(&mut *connection)
        .await?;
    row.map(|row| {
        let values = parse_settings(&row.value).ok_or_else(|| corrupt("settings"))?;
        Ok(StoredSettings {
            values,
            version: Version::from_stored(row.version),
        })
    })
    .transpose()
}

async fn feed_id_of(
    connection: &mut SqliteConnection,
    id: &ThemeId,
) -> Result<Option<Uuid>, StoreError> {
    let name = id.as_str();
    Ok(sqlx::query_scalar!(
        r#"SELECT feed_id AS "feed_id: Uuid" FROM installed_themes WHERE theme_id = ?"#,
        name
    )
    .fetch_optional(&mut *connection)
    .await?)
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

    async fn settings(&mut self, id: &ThemeId) -> Result<Option<StoredSettings>, AppError> {
        Ok(stored_settings(&mut self.0, id).await?)
    }

    async fn put_settings(
        &mut self,
        id: &ThemeId,
        values: &ThemeSettingValues,
        expected: Option<Version>,
        at: Timestamp,
    ) -> Result<(Version, Change), AppError> {
        let Some(feed_id) = feed_id_of(&mut self.0, id).await? else {
            return Err(theme_not_found(id.as_str()));
        };
        let key = settings_key(id);
        let json = settings_json(values).to_string();
        let millis = at.as_millis();
        let conflict = || AppError::version_conflict("theme settings", id);
        let (version, op) = if let Some(expected) = expected {
            let next = expected.next();
            let (next_value, expected_value) = (next.value(), expected.value());
            let updated = sqlx::query!(
                "UPDATE settings SET value = ?, version = ?, updated_at = ?
                 WHERE key = ? AND version = ?",
                json,
                next_value,
                millis,
                key,
                expected_value,
            )
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
            if updated.rows_affected() == 0 {
                return Err(conflict());
            }
            (next, ChangeOp::Update)
        } else {
            let inserted = sqlx::query!(
                "INSERT INTO settings (key, value, version, updated_at) VALUES (?, ?, 1, ?)
                 ON CONFLICT (key) DO NOTHING",
                key,
                json,
                millis,
            )
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
            if inserted.rows_affected() == 0 {
                return Err(conflict());
            }
            (Version::INITIAL, ChangeOp::Insert)
        };
        let change = change_feed::record(&mut self.0, Entity::Setting, feed_id, op, at).await?;
        Ok((version, change))
    }

    async fn delete_settings(
        &mut self,
        id: &ThemeId,
        expected: Version,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let Some(stored) = stored_settings(&mut self.0, id).await? else {
            return Ok(None);
        };
        if stored.version != expected {
            return Err(AppError::version_conflict("theme settings", id));
        }
        let key = settings_key(id);
        let expected_value = expected.value();
        let deleted = sqlx::query!(
            "DELETE FROM settings WHERE key = ? AND version = ?",
            key,
            expected_value
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        if deleted.rows_affected() == 0 {
            return Err(AppError::version_conflict("theme settings", id));
        }
        self.settings_removed(id, at).await
    }

    async fn purge_settings(
        &mut self,
        id: &ThemeId,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let key = settings_key(id);
        let deleted = sqlx::query!("DELETE FROM settings WHERE key = ?", key)
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
        if deleted.rows_affected() == 0 {
            return Ok(None);
        }
        self.settings_removed(id, at).await
    }

    async fn set_origin(
        &mut self,
        id: &ThemeId,
        source: Option<&SourceId>,
        at: Timestamp,
    ) -> Result<(), AppError> {
        let key = origin_key(id);
        let Some(source) = source else {
            sqlx::query!("DELETE FROM settings WHERE key = ?", key)
                .execute(&mut *self.0)
                .await
                .map_err(database)?;
            return Ok(());
        };
        let value = serde_json::to_string(source.as_str()).map_err(|_| corrupt("origin"))?;
        let millis = at.as_millis();
        sqlx::query!(
            "INSERT INTO settings (key, value, version, updated_at) VALUES (?, ?, 1, ?)
             ON CONFLICT (key) DO UPDATE
                 SET value = excluded.value, version = version + 1,
                     updated_at = excluded.updated_at",
            key,
            value,
            millis,
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        Ok(())
    }

    async fn code_enabled(&mut self, id: &ThemeId) -> Result<bool, AppError> {
        let key = code_key(id);
        let row = sqlx::query_scalar!("SELECT key FROM settings WHERE key = ?", key)
            .fetch_optional(&mut *self.0)
            .await
            .map_err(database)?;
        Ok(row.is_some())
    }

    async fn set_code(
        &mut self,
        id: &ThemeId,
        enabled: bool,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let key = code_key(id);
        let changed = if enabled {
            let millis = at.as_millis();
            sqlx::query!(
                "INSERT INTO settings (key, value, version, updated_at) VALUES (?, 'true', 1, ?)
                 ON CONFLICT (key) DO NOTHING",
                key,
                millis,
            )
            .execute(&mut *self.0)
            .await
            .map_err(database)?
            .rows_affected()
                > 0
        } else {
            sqlx::query!("DELETE FROM settings WHERE key = ?", key)
                .execute(&mut *self.0)
                .await
                .map_err(database)?
                .rows_affected()
                > 0
        };
        if !changed {
            return Ok(None);
        }
        let Some(feed_id) = feed_id_of(&mut self.0, id).await? else {
            return Ok(None);
        };
        Ok(Some(
            change_feed::record(&mut self.0, Entity::Setting, feed_id, ChangeOp::Update, at)
                .await?,
        ))
    }

    async fn commit(self: Box<Self>) -> Result<(), AppError> {
        self.0.commit().await.map_err(database)
    }
}

impl Write {
    /// Records the removal of a theme's values under the id the theme has in the feed.
    async fn settings_removed(
        &mut self,
        id: &ThemeId,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        let Some(feed_id) = feed_id_of(&mut self.0, id).await? else {
            return Ok(None);
        };
        Ok(Some(
            change_feed::record(&mut self.0, Entity::Setting, feed_id, ChangeOp::Delete, at)
                .await?,
        ))
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

    async fn settings(&self, id: &ThemeId) -> Result<Option<StoredSettings>, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        Ok(stored_settings(&mut read, id).await?)
    }

    async fn origins(&self) -> Result<BTreeMap<ThemeId, SourceId>, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        let rows = sqlx::query!("SELECT key, value FROM settings WHERE key LIKE 'theme-origin.%'")
            .fetch_all(&mut *read)
            .await
            .map_err(database)?;
        let mut origins = BTreeMap::new();
        for row in rows {
            let id = row
                .key
                .strip_prefix(ORIGIN_PREFIX)
                .and_then(|id| ThemeId::parse(id).ok());
            let source = serde_json::from_str::<String>(&row.value)
                .ok()
                .and_then(|text| SourceId::parse(&text));
            let (Some(id), Some(source)) = (id, source) else {
                return Err(corrupt("origin").into());
            };
            origins.insert(id, source);
        }
        Ok(origins)
    }

    async fn code_switches(&self) -> Result<BTreeSet<ThemeId>, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        let rows = sqlx::query_scalar!("SELECT key FROM settings WHERE key LIKE 'theme-code.%'")
            .fetch_all(&mut *read)
            .await
            .map_err(database)?;
        rows.iter()
            .map(|key| {
                key.strip_prefix(CODE_PREFIX)
                    .and_then(|id| ThemeId::parse(id).ok())
                    .ok_or_else(|| corrupt("code switch").into())
            })
            .collect()
    }
}
