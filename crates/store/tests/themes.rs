#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::theme_package::{PackageFile, parse_manifest};
use nullradix_app::themes::{ThemeFiles, ThemeRepository, ThemeService};
use nullradix_app::{ChangeFeed, ChangeOp, ChangeSignal, Entity, SystemClock, UuidV7Generator};
use nullradix_domain::{InstalledTheme, ThemeId, Timestamp, Version};
use nullradix_store::Store;
use std::sync::Mutex;
use uuid::Uuid;

const MANIFEST: &str = r##"{
    "id": "night-blue", "name": "Night Blue", "version": "1.0.0", "author": "Ada",
    "sdkVersion": "^1.3", "colorScheme": "dark", "startColor": "#101018",
    "preview": "assets/preview.png", "slots": ["Progress"],
    "permissions": { "network": ["*.cdn.test"] }, "claudeTheme": true,
    "settingsSchema": {
        "dim": { "type": "boolean", "label": "Dim", "hint": "less light", "default": true },
        "mode": { "type": "select", "label": "Mode", "default": "a",
                  "options": [{ "value": "a", "label": "A" }, { "value": "b", "label": "B" }] }
    }
}"##;

fn theme(json: &str, at: i64) -> InstalledTheme {
    InstalledTheme::install(parse_manifest(json).unwrap(), Timestamp::from_millis(at))
}

async fn changes(store: &Store) -> Vec<(Entity, ChangeOp)> {
    store
        .changes_since(0, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|change| (change.entity, change.op))
        .collect()
}

#[tokio::test]
async fn a_theme_round_trips_with_every_manifest_field() {
    let store = Store::in_memory().await.unwrap();
    let installed = theme(MANIFEST, 5);
    let mut write = store.begin().await.unwrap();
    let feed = Uuid::from_u128(9);
    let change = write.insert(&installed, feed).await.unwrap();
    write.commit().await.unwrap();
    assert_eq!(
        (change.entity, change.op, change.entity_id),
        (Entity::Theme, ChangeOp::Insert, feed)
    );

    let id = ThemeId::parse("night-blue").unwrap();
    let stored = store.get(&id).await.unwrap().unwrap();
    assert_eq!(stored, installed);
    assert_eq!(store.list().await.unwrap(), vec![installed]);
    assert!(
        store
            .get(&ThemeId::parse("other").unwrap())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn themes_list_sorted_by_id() {
    let store = Store::in_memory().await.unwrap();
    let mut write = store.begin().await.unwrap();
    for (index, id) in ["zebra", "alpha", "mid-way"].into_iter().enumerate() {
        let json = MANIFEST.replace("night-blue", id);
        write
            .insert(&theme(&json, 1), Uuid::from_u128(index as u128 + 1))
            .await
            .unwrap();
    }
    write.commit().await.unwrap();
    let ids: Vec<String> = store
        .list()
        .await
        .unwrap()
        .iter()
        .map(|theme| theme.id().to_string())
        .collect();
    assert_eq!(ids, ["alpha", "mid-way", "zebra"]);
}

#[tokio::test]
async fn updates_check_the_version_and_keep_the_feed_id() {
    let store = Store::in_memory().await.unwrap();
    let first = theme(MANIFEST, 1);
    let mut write = store.begin().await.unwrap();
    let feed = Uuid::from_u128(4);
    write.insert(&first, feed).await.unwrap();
    write.commit().await.unwrap();

    let next = first.reinstalled(
        parse_manifest(&MANIFEST.replace("1.0.0", "1.1.0")).unwrap(),
        Timestamp::from_millis(7),
    );
    let mut write = store.begin().await.unwrap();
    let change = write.update(&next, first.record().version).await.unwrap();
    assert_eq!(
        (change.entity, change.op, change.entity_id),
        (Entity::Theme, ChangeOp::Update, feed)
    );
    write.commit().await.unwrap();
    let id = ThemeId::parse("night-blue").unwrap();
    assert_eq!(store.get(&id).await.unwrap().unwrap(), next);

    let mut write = store.begin().await.unwrap();
    let error = write.update(&next, Version::INITIAL).await.unwrap_err();
    assert_eq!(error.code(), "version-conflict");
    let ghost = theme(&MANIFEST.replace("night-blue", "ghost"), 1);
    let error = write.update(&ghost, Version::INITIAL).await.unwrap_err();
    assert_eq!(error.code(), "theme-not-found");
}

#[tokio::test]
async fn deleting_checks_the_version_and_reports_missing_themes() {
    let store = Store::in_memory().await.unwrap();
    let installed = theme(MANIFEST, 1);
    let id = ThemeId::parse("night-blue").unwrap();
    let mut write = store.begin().await.unwrap();
    assert!(
        write
            .delete(&id, Version::INITIAL, Timestamp::from_millis(2))
            .await
            .unwrap()
            .is_none()
    );
    write.insert(&installed, Uuid::from_u128(3)).await.unwrap();
    let error = write
        .delete(&id, Version::from_stored(5), Timestamp::from_millis(2))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "version-conflict");
    let change = write
        .delete(&id, Version::INITIAL, Timestamp::from_millis(2))
        .await
        .unwrap()
        .unwrap();
    write.commit().await.unwrap();
    assert_eq!(
        (change.entity, change.op),
        (Entity::Theme, ChangeOp::Delete)
    );
    assert_eq!(store.list().await.unwrap().len(), 0);
}

#[tokio::test]
async fn the_choice_persists_and_unchanged_choices_leave_no_trace() {
    let store = Store::in_memory().await.unwrap();
    assert_eq!(store.selection().await.unwrap(), None);
    let at = Timestamp::from_millis(3);
    let mut write = store.begin().await.unwrap();
    assert!(write.select("dark", at).await.unwrap().is_some());
    assert_eq!(write.selection().await.unwrap().as_deref(), Some("dark"));
    assert!(write.select("dark", at).await.unwrap().is_none());
    assert!(write.select("light", at).await.unwrap().is_some());
    write.commit().await.unwrap();
    assert_eq!(store.selection().await.unwrap().as_deref(), Some("light"));
    assert_eq!(
        changes(&store).await,
        [
            (Entity::Setting, ChangeOp::Update),
            (Entity::Setting, ChangeOp::Update)
        ]
    );
}

#[tokio::test]
async fn a_rolled_back_write_changes_nothing() {
    let store = Store::in_memory().await.unwrap();
    let mut write = store.begin().await.unwrap();
    write
        .insert(&theme(MANIFEST, 1), Uuid::from_u128(1))
        .await
        .unwrap();
    write
        .select("light", Timestamp::from_millis(1))
        .await
        .unwrap();
    drop(write);
    assert_eq!(store.list().await.unwrap().len(), 0);
    assert_eq!(store.selection().await.unwrap(), None);
    assert_eq!(changes(&store).await.len(), 0);
}

#[derive(Default)]
struct Files(Mutex<HashMap<String, Vec<PackageFile>>>);

#[async_trait]
impl ThemeFiles for Files {
    async fn unpack(&self, archive: Vec<u8>) -> Result<Vec<PackageFile>, AppError> {
        let text = String::from_utf8(archive).unwrap();
        Ok(text
            .split("\n--\n")
            .filter_map(|part| part.split_once('='))
            .map(|(path, data)| PackageFile {
                path: path.to_owned(),
                data: data.as_bytes().to_vec(),
            })
            .collect())
    }

    async fn store(&self, id: &ThemeId, files: Vec<PackageFile>) -> Result<(), AppError> {
        self.0.lock().unwrap().insert(id.to_string(), files);
        Ok(())
    }

    async fn remove(&self, id: &ThemeId) -> Result<(), AppError> {
        self.0.lock().unwrap().remove(id.as_str());
        Ok(())
    }

    async fn read(&self, _id: &ThemeId, _path: &[String]) -> Result<Option<Vec<u8>>, AppError> {
        Ok(None)
    }
}

#[tokio::test]
async fn the_service_installs_selects_and_removes_against_the_database() {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let files = Arc::new(Files::default());
    let service = ThemeService::new(
        store.clone(),
        files.clone(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
    );
    let archive = format!(
        "manifest.json={MANIFEST}\n--\ntheme.css=a {{ color: red }}\n--\nclaude/hooks/hooks.json={{}}"
    )
    .into_bytes();
    let installed = service.install(archive.clone()).await.unwrap();
    service.select("night-blue").await.unwrap();
    assert_eq!(service.list().await.unwrap().active, "night-blue");

    let again = service.install(archive).await.unwrap();
    assert_eq!(again.record().version, Version::from_stored(2));
    assert_eq!(again.record().installed_at, installed.record().installed_at);

    service
        .remove("night-blue", again.record().version)
        .await
        .unwrap();
    let list = service.list().await.unwrap();
    assert_eq!(list.themes.len(), 0);
    assert_eq!(list.active, "nullradix");
    assert_eq!(
        store.selection().await.unwrap().as_deref(),
        Some("nullradix")
    );
    assert!(files.0.lock().unwrap().is_empty());
    assert_eq!(
        changes(&store).await,
        [
            (Entity::Theme, ChangeOp::Insert),
            (Entity::Setting, ChangeOp::Update),
            (Entity::Theme, ChangeOp::Update),
            (Entity::Theme, ChangeOp::Delete),
            (Entity::Setting, ChangeOp::Update),
        ]
    );
}
