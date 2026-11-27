#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::theme_package::{PackageFile, parse_manifest};
use nullradix_app::themes::{ThemeFiles, ThemeRepository, ThemeService};
use nullradix_app::{ChangeFeed, ChangeOp, ChangeSignal, Entity, SystemClock, UuidV7Generator};
use nullradix_domain::{
    InstalledTheme, SettingValue, SourceId, ThemeId, ThemeSettingValues, Timestamp, Version,
};
use nullradix_store::Store;
use std::collections::BTreeMap;
use std::sync::Mutex;
use uuid::Uuid;

const MANIFEST: &str = r##"{
    "id": "night-blue", "name": "Night Blue", "version": "1.0.0", "author": "Ada",
    "sdkVersion": "^2.0", "colorScheme": "dark", "startColor": "#101018",
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

    async fn paths(&self, _id: &ThemeId) -> Option<Vec<String>> {
        Some(Vec::new())
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
    let installed = service
        .install(archive.clone(), true, true)
        .await
        .unwrap()
        .theme;
    service.select("night-blue", true).await.unwrap();
    assert_eq!(service.list().await.unwrap().active, "night-blue");

    let again = service.install(archive, true, true).await.unwrap().theme;
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

fn input(items: &[(&str, SettingValue)]) -> BTreeMap<String, SettingValue> {
    items
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

fn values(items: &[(&str, SettingValue)]) -> ThemeSettingValues {
    ThemeSettingValues::restore(input(items))
}

fn choice(text: &str) -> SettingValue {
    SettingValue::Choice(text.to_owned())
}

#[tokio::test]
async fn settings_round_trip_with_their_version_and_feed_entries() {
    let store = Store::in_memory().await.unwrap();
    let id = ThemeId::parse("night-blue").unwrap();
    let feed = Uuid::from_u128(7);
    let at = Timestamp::from_millis(4);
    let mut write = store.begin().await.unwrap();
    write.insert(&theme(MANIFEST, 1), feed).await.unwrap();
    assert_eq!(write.settings(&id).await.unwrap(), None);
    let first = values(&[("dim", SettingValue::Switch(false))]);
    let (version, change) = write.put_settings(&id, &first, None, at).await.unwrap();
    assert_eq!(version, Version::INITIAL);
    assert_eq!(
        (change.entity, change.op, change.entity_id),
        (Entity::Setting, ChangeOp::Insert, feed)
    );
    let second = values(&[("mode", choice("b"))]);
    let (version, change) = write
        .put_settings(&id, &second, Some(Version::INITIAL), at)
        .await
        .unwrap();
    assert_eq!(
        (version, change.op),
        (Version::from_stored(2), ChangeOp::Update)
    );
    write.commit().await.unwrap();

    let stored = store.settings(&id).await.unwrap().unwrap();
    assert_eq!((stored.values, stored.version), (second, version));
    let mut write = store.begin().await.unwrap();
    let change = write
        .delete_settings(&id, version, at)
        .await
        .unwrap()
        .unwrap();
    write.commit().await.unwrap();
    assert_eq!(
        (change.entity, change.op, change.entity_id),
        (Entity::Setting, ChangeOp::Delete, feed)
    );
    assert_eq!(store.settings(&id).await.unwrap(), None);
}

#[tokio::test]
async fn settings_writes_are_guarded_by_the_version() {
    let store = Store::in_memory().await.unwrap();
    let id = ThemeId::parse("night-blue").unwrap();
    let at = Timestamp::from_millis(4);
    let on = values(&[("dim", SettingValue::Switch(true))]);
    let mut write = store.begin().await.unwrap();
    write
        .insert(&theme(MANIFEST, 1), Uuid::from_u128(1))
        .await
        .unwrap();
    let missing = Version::from_stored(1);
    let error = write
        .put_settings(&id, &on, Some(missing), at)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "version-conflict");
    write.put_settings(&id, &on, None, at).await.unwrap();
    let error = write.put_settings(&id, &on, None, at).await.unwrap_err();
    assert_eq!(error.code(), "version-conflict");
    let stale = Version::from_stored(4);
    let error = write
        .put_settings(&id, &on, Some(stale), at)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "version-conflict");
    let error = write.delete_settings(&id, stale, at).await.unwrap_err();
    assert_eq!(error.code(), "version-conflict");
    let ghost = ThemeId::parse("ghost").unwrap();
    let error = write.put_settings(&ghost, &on, None, at).await.unwrap_err();
    assert_eq!(error.code(), "theme-not-found");
    let none = write.delete_settings(&ghost, stale, at).await.unwrap();
    assert!(none.is_none());
    write.commit().await.unwrap();
    assert_eq!(changes(&store).await.len(), 2);
}

#[tokio::test]
async fn purging_drops_the_values_of_one_theme_only() {
    let store = Store::in_memory().await.unwrap();
    let at = Timestamp::from_millis(4);
    let on = values(&[("dim", SettingValue::Switch(true))]);
    let mut write = store.begin().await.unwrap();
    for (index, name) in ["night-blue", "other"].into_iter().enumerate() {
        let json = MANIFEST.replace("night-blue", name);
        write
            .insert(&theme(&json, 1), Uuid::from_u128(index as u128 + 1))
            .await
            .unwrap();
        let id = ThemeId::parse(name).unwrap();
        write.put_settings(&id, &on, None, at).await.unwrap();
    }
    let id = ThemeId::parse("night-blue").unwrap();
    assert!(write.purge_settings(&id, at).await.unwrap().is_some());
    assert!(write.purge_settings(&id, at).await.unwrap().is_none());
    write.commit().await.unwrap();
    assert_eq!(store.settings(&id).await.unwrap(), None);
    let other = ThemeId::parse("other").unwrap();
    assert!(store.settings(&other).await.unwrap().is_some());
}

#[tokio::test]
async fn an_origin_is_kept_replaced_and_dropped_with_the_write() {
    let store = Store::in_memory().await.unwrap();
    let at = Timestamp::from_millis(4);
    let id = ThemeId::parse("night-blue").unwrap();
    let one = SourceId::parse("aaaaaaaaaaaa").unwrap();
    let two = SourceId::parse("bbbbbbbbbbbb").unwrap();
    assert!(store.origins().await.unwrap().is_empty());
    let mut write = store.begin().await.unwrap();
    write.set_origin(&id, Some(&one), at).await.unwrap();
    write.set_origin(&id, Some(&two), at).await.unwrap();
    write.commit().await.unwrap();
    assert_eq!(store.origins().await.unwrap().get(&id), Some(&two));
    let mut write = store.begin().await.unwrap();
    write.set_origin(&id, None, at).await.unwrap();
    drop(write);
    assert_eq!(store.origins().await.unwrap().len(), 1);
    let mut write = store.begin().await.unwrap();
    write.set_origin(&id, None, at).await.unwrap();
    write.commit().await.unwrap();
    assert!(store.origins().await.unwrap().is_empty());
}

#[tokio::test]
async fn the_code_switch_is_a_row_that_exists_only_while_it_is_on() {
    let store = Store::in_memory().await.unwrap();
    let at = Timestamp::from_millis(4);
    let id = ThemeId::parse("night-blue").unwrap();
    let other = ThemeId::parse("other-theme").unwrap();
    let feed = Uuid::from_u128(11);
    let mut write = store.begin().await.unwrap();
    write.insert(&theme(MANIFEST, 1), feed).await.unwrap();
    write.commit().await.unwrap();
    assert!(store.code_switches().await.unwrap().is_empty());

    let mut write = store.begin().await.unwrap();
    assert!(!write.code_enabled(&id).await.unwrap());
    let on = write.set_code(&id, true, at).await.unwrap().unwrap();
    assert_eq!(
        (on.entity, on.op, on.entity_id),
        (Entity::Setting, ChangeOp::Update, feed)
    );
    assert!(write.code_enabled(&id).await.unwrap());
    // already on is no change, and a theme that is not installed gets no row
    assert!(write.set_code(&id, true, at).await.unwrap().is_none());
    assert!(write.set_code(&other, true, at).await.unwrap().is_none());
    write.commit().await.unwrap();
    let switched = store.code_switches().await.unwrap();
    assert_eq!(switched.len(), 1);
    assert!(switched.contains(&id));

    let mut write = store.begin().await.unwrap();
    assert!(write.set_code(&id, false, at).await.unwrap().is_some());
    assert!(write.set_code(&id, false, at).await.unwrap().is_none());
    drop(write);
    assert!(store.code_switches().await.unwrap().contains(&id));
    let mut write = store.begin().await.unwrap();
    write.set_code(&id, false, at).await.unwrap();
    write.set_code(&other, false, at).await.unwrap();
    write.commit().await.unwrap();
    assert!(store.code_switches().await.unwrap().is_empty());
}

fn archive() -> Vec<u8> {
    let hooks = "claude/hooks/hooks.json={}";
    format!("manifest.json={MANIFEST}\n--\ntheme.css=a {{ color: red }}\n--\n{hooks}").into_bytes()
}

fn service(store: &Arc<Store>) -> ThemeService {
    ThemeService::new(
        store.clone(),
        Arc::new(Files::default()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
    )
}

#[tokio::test]
async fn the_service_saves_and_drops_settings_with_the_theme() {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let service = service(&store);
    service.install(archive(), true, true).await.unwrap();
    let saved = service
        .save_settings("night-blue", input(&[("mode", choice("b"))]), None)
        .await
        .unwrap();
    assert_eq!(saved.version, Some(Version::INITIAL));
    let error = service
        .save_settings("night-blue", input(&[("mode", choice("c"))]), saved.version)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "theme-settings-invalid");
    assert_eq!(service.settings("night-blue").await.unwrap(), saved);

    // a reinstall keeps what was chosen
    let again = service.install(archive(), true, true).await.unwrap().theme;
    assert_eq!(service.settings("night-blue").await.unwrap(), saved);

    // a refused removal leaves the values where they were
    let error = service
        .remove("night-blue", Version::from_stored(9))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "version-conflict");
    assert_eq!(service.settings("night-blue").await.unwrap(), saved);

    service
        .remove("night-blue", again.record().version)
        .await
        .unwrap();
    let id = ThemeId::parse("night-blue").unwrap();
    assert_eq!(store.settings(&id).await.unwrap(), None);
    service.install(archive(), true, true).await.unwrap();
    assert_eq!(service.settings("night-blue").await.unwrap().version, None);
}

#[tokio::test]
async fn resetting_removes_the_row_and_reports_a_stale_version() {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let service = service(&store);
    service.install(archive(), true, true).await.unwrap();
    service.reset_settings("night-blue", None).await.unwrap();
    let off = input(&[("dim", SettingValue::Switch(false))]);
    let saved = service
        .save_settings("night-blue", off, None)
        .await
        .unwrap();
    let error = service
        .reset_settings("night-blue", Some(Version::from_stored(2)))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "version-conflict");
    service
        .reset_settings("night-blue", saved.version)
        .await
        .unwrap();
    let id = ThemeId::parse("night-blue").unwrap();
    assert_eq!(store.settings(&id).await.unwrap(), None);
}

#[tokio::test]
async fn two_saves_from_the_same_version_let_exactly_one_win() {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let service = service(&store);
    service.install(archive(), true, true).await.unwrap();
    let (first, second) = tokio::join!(
        service.save_settings("night-blue", input(&[("mode", choice("a"))]), None),
        service.save_settings("night-blue", input(&[("mode", choice("b"))]), None),
    );
    let codes: Vec<_> = [first, second]
        .into_iter()
        .map(|result| result.err().map(|error| error.code()))
        .collect();
    assert_eq!(codes.iter().filter(|code| code.is_none()).count(), 1);
    assert!(codes.contains(&Some("version-conflict")));
}

#[tokio::test]
async fn a_save_after_the_uninstall_finds_no_theme() {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let service = service(&store);
    let installed = service.install(archive(), true, true).await.unwrap().theme;
    service
        .remove("night-blue", installed.record().version)
        .await
        .unwrap();
    let on = input(&[("dim", SettingValue::Switch(true))]);
    let error = service
        .save_settings("night-blue", on, None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "theme-not-found");
}
