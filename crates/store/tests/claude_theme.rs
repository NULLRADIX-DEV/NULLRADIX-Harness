#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nullradix_app::accounts::AccountRepository;
use nullradix_app::claude_theme::{
    ClaudeSlots, ClaudeThemeRepository, ClaudeThemeService, SlotApplied, SlotFailure, SlotState,
    SlotTheme, TargetState,
};
use nullradix_app::theme_package::PackageFile;
use nullradix_app::themes::{ThemeFiles, ThemeService};
use nullradix_app::{AppError, ChangeFeed, ChangeSignal, Entity, SystemClock, UuidV7Generator};
use nullradix_domain::{
    Account, AccountName, AccountSlug, ConfigDir, Id, SettingValue, ThemeId, Timestamp,
};
use nullradix_store::Store;
use uuid::Uuid;

const CLAUDE_MANIFEST: &str = r#"{
    "id": "night-blue", "name": "Night Blue", "version": "1.0.0", "author": "Ada",
    "sdkVersion": "^1.3", "colorScheme": "dark", "slots": [], "claudeTheme": true,
    "settingsSchema": {
        "dim": { "type": "boolean", "label": "Dim", "default": true },
        "mode": { "type": "select", "label": "Mode", "default": "a",
                  "options": [{ "value": "a", "label": "A" }, { "value": "b", "label": "B" }] }
    }
}"#;

#[derive(Default)]
struct Files;

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

    async fn store(&self, _id: &ThemeId, _files: Vec<PackageFile>) -> Result<(), AppError> {
        Ok(())
    }

    async fn remove(&self, _id: &ThemeId) -> Result<(), AppError> {
        Ok(())
    }

    async fn read(&self, _id: &ThemeId, _path: &[String]) -> Result<Option<Vec<u8>>, AppError> {
        Ok(None)
    }

    async fn paths(&self, _id: &ThemeId) -> Option<Vec<String>> {
        Some(Vec::new())
    }
}

/// Folders kept in memory; `foreign` and `broken` ones refuse a write.
#[derive(Default)]
struct Slots {
    held: Mutex<BTreeMap<String, SlotTheme>>,
    foreign: Mutex<BTreeSet<String>>,
    broken: Mutex<BTreeSet<String>>,
    writes: Mutex<usize>,
}

impl Slots {
    fn holds(&self, dir: &str) -> Option<SlotTheme> {
        self.held.lock().unwrap().get(dir).cloned()
    }

    fn writes(&self) -> usize {
        *self.writes.lock().unwrap()
    }

    fn refuse(&self, dir: &str) -> Result<(), SlotFailure> {
        if self.foreign.lock().unwrap().contains(dir) {
            return Err(SlotFailure::Foreign(dir.to_owned()));
        }
        if self.broken.lock().unwrap().contains(dir) {
            return Err(SlotFailure::Failed(String::from("disk full")));
        }
        Ok(())
    }
}

#[async_trait]
impl ClaudeSlots for Slots {
    fn default_dir(&self) -> Option<String> {
        Some(String::from("/home/.claude"))
    }

    fn key(&self, dir: &str) -> String {
        dir.to_lowercase()
    }

    fn settings_hash(&self, theme: &SlotTheme) -> String {
        format!("{:?}", theme.settings)
    }

    async fn inspect(&self, dir: &str) -> Result<SlotState, SlotFailure> {
        if self.foreign.lock().unwrap().contains(dir) {
            return Ok(SlotState::Foreign);
        }
        Ok(self
            .holds(dir)
            .map_or(SlotState::Empty, |theme| SlotState::Managed {
                theme_id: theme.id.clone(),
                theme_version: theme.version.clone(),
                settings_hash: self.settings_hash(&theme),
            }))
    }

    async fn apply(&self, dir: &str, theme: &SlotTheme) -> Result<SlotApplied, SlotFailure> {
        self.refuse(dir)?;
        let changed = self.holds(dir).as_ref() != Some(theme);
        if changed {
            *self.writes.lock().unwrap() += 1;
            self.held
                .lock()
                .unwrap()
                .insert(dir.to_owned(), theme.clone());
        }
        Ok(SlotApplied {
            written: usize::from(changed),
            removed: 0,
        })
    }

    async fn clear(&self, dir: &str) -> Result<bool, SlotFailure> {
        self.refuse(dir)?;
        Ok(self.held.lock().unwrap().remove(dir).is_some())
    }
}

struct Fixture {
    store: Arc<Store>,
    themes: Arc<ThemeService>,
    slots: Arc<Slots>,
    service: ClaudeThemeService,
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let themes = Arc::new(ThemeService::new(
        store.clone(),
        Arc::new(Files),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let slots = Arc::new(Slots::default());
    let service = ClaudeThemeService::new(
        store.clone(),
        themes.clone(),
        store.clone(),
        slots.clone(),
        Arc::new(SystemClock),
        signal,
    );
    Fixture {
        store,
        themes,
        slots,
        service,
    }
}

fn package(id: &str, claude: bool) -> Vec<u8> {
    let manifest = CLAUDE_MANIFEST.replace("night-blue", id).replace(
        r#""claudeTheme": true"#,
        &format!(r#""claudeTheme": {claude}"#),
    );
    let mut text = format!("manifest.json={manifest}\n--\ntheme.css=a {{ color: red }}");
    if claude {
        text.push_str("\n--\nclaude/hooks/hooks.json={}");
    }
    text.into_bytes()
}

async fn add_account(store: &Store, slug: &str, dir: &str, position: i64) -> Account {
    let account = Account::new(
        Id::from_uuid(Uuid::now_v7()),
        AccountSlug::parse(slug).unwrap(),
        AccountName::parse(slug).unwrap(),
        ConfigDir::parse(dir).unwrap(),
        position,
        Timestamp::from_millis(1),
    );
    AccountRepository::insert(store, &account).await.unwrap();
    account
}

async fn feed(store: &Store) -> Vec<Entity> {
    store
        .changes_since(0, 1000)
        .await
        .unwrap()
        .into_iter()
        .map(|change| change.entity)
        .collect()
}

#[tokio::test]
async fn installing_and_choosing_a_theme_never_applies_its_claude_part() {
    let f = fixture().await;
    add_account(&f.store, "work", "/accounts/work", 1).await;
    f.themes
        .install(package("night-blue", true), true, true)
        .await
        .unwrap();
    f.themes.select("night-blue", true).await.unwrap();
    let status = f.service.reconcile().await.unwrap();
    assert!(!status.enabled);
    assert_eq!(status.state, TargetState::None);
    assert_eq!(f.slots.writes(), 0);
    assert!(f.slots.holds("/home/.claude").is_none());
}

#[tokio::test]
async fn the_switch_puts_the_theme_into_the_default_folder_and_every_account() {
    let f = fixture().await;
    add_account(&f.store, "work", "/accounts/work", 1).await;
    add_account(&f.store, "spare", "/accounts/spare", 2).await;
    f.themes
        .install(package("night-blue", true), true, true)
        .await
        .unwrap();
    f.themes.select("night-blue", true).await.unwrap();

    let status = f.service.set_enabled(true).await.unwrap();

    assert_eq!(status.state, TargetState::Active);
    assert_eq!(status.theme_id.as_deref(), Some("night-blue"));
    let paths: Vec<&str> = status.targets.iter().map(|t| t.path.as_str()).collect();
    assert_eq!(
        paths,
        ["/home/.claude", "/accounts/work", "/accounts/spare"]
    );
    for dir in paths {
        let held = f.slots.holds(dir).unwrap();
        assert_eq!(held.settings["dim"], SettingValue::Switch(true));
        assert_eq!(held.settings["mode"], SettingValue::Choice("a".into()));
    }
    assert!(feed(&f.store).await.contains(&Entity::Setting));
    assert!(f.store.enabled().await.unwrap());
}

#[tokio::test]
async fn a_second_reconcile_writes_nothing() {
    let f = fixture().await;
    f.themes
        .install(package("night-blue", true), true, true)
        .await
        .unwrap();
    f.themes.select("night-blue", true).await.unwrap();
    f.service.set_enabled(true).await.unwrap();
    let writes = f.slots.writes();
    f.service.reconcile().await.unwrap();
    assert_eq!(f.slots.writes(), writes);
}

#[tokio::test]
async fn saving_theme_settings_and_reinstalling_carry_over() {
    let f = fixture().await;
    f.themes
        .install(package("night-blue", true), true, true)
        .await
        .unwrap();
    f.themes.select("night-blue", true).await.unwrap();
    f.service.set_enabled(true).await.unwrap();

    let values = BTreeMap::from([
        (String::from("mode"), SettingValue::Choice("b".into())),
        (String::from("dim"), SettingValue::Switch(false)),
    ]);
    f.themes
        .save_settings("night-blue", values, None)
        .await
        .unwrap();
    f.service.reconcile().await.unwrap();
    let held = f.slots.holds("/home/.claude").unwrap();
    assert_eq!(held.settings["mode"], SettingValue::Choice("b".into()));
    assert_eq!(held.settings["dim"], SettingValue::Switch(false));

    let newer = String::from_utf8(package("night-blue", true))
        .unwrap()
        .replace(r#""version": "1.0.0""#, r#""version": "1.1.0""#);
    f.themes
        .install(newer.into_bytes(), true, true)
        .await
        .unwrap();
    f.service.reconcile().await.unwrap();
    assert_eq!(f.slots.holds("/home/.claude").unwrap().version, "1.1.0");
}

#[tokio::test]
async fn a_folder_written_with_older_settings_is_not_reported_active() {
    let f = fixture().await;
    f.themes
        .install(package("night-blue", true), true, true)
        .await
        .unwrap();
    f.themes.select("night-blue", true).await.unwrap();
    assert_eq!(
        f.service.set_enabled(true).await.unwrap().state,
        TargetState::Active
    );
    let values = BTreeMap::from([(String::from("dim"), SettingValue::Switch(false))]);
    f.themes
        .save_settings("night-blue", values, None)
        .await
        .unwrap();
    assert_eq!(f.service.status().await.unwrap().state, TargetState::Error);
    assert_eq!(
        f.service.reconcile().await.unwrap().state,
        TargetState::Active
    );
}

#[tokio::test]
async fn switching_off_or_to_another_theme_takes_it_out_again() {
    let f = fixture().await;
    add_account(&f.store, "work", "/accounts/work", 1).await;
    f.themes
        .install(package("night-blue", true), true, true)
        .await
        .unwrap();
    f.themes
        .install(package("plain", false), false, true)
        .await
        .unwrap();
    f.themes.select("night-blue", true).await.unwrap();
    f.service.set_enabled(true).await.unwrap();

    f.themes.select("plain", true).await.unwrap();
    let status = f.service.reconcile().await.unwrap();
    assert_eq!(status.state, TargetState::None);
    assert!(f.slots.holds("/home/.claude").is_none());
    assert!(f.slots.holds("/accounts/work").is_none());

    f.themes.select("night-blue", true).await.unwrap();
    f.service.reconcile().await.unwrap();
    assert!(f.slots.holds("/accounts/work").is_some());
    f.service.set_enabled(false).await.unwrap();
    assert!(f.slots.holds("/accounts/work").is_none());
    assert_eq!(f.store.targets().await.unwrap(), Vec::<String>::new());
}

#[tokio::test]
async fn removing_the_chosen_theme_takes_it_out() {
    let f = fixture().await;
    let installed = f
        .themes
        .install(package("night-blue", true), true, true)
        .await
        .unwrap()
        .theme;
    f.themes.select("night-blue", true).await.unwrap();
    f.service.set_enabled(true).await.unwrap();
    f.themes
        .remove("night-blue", installed.record().version)
        .await
        .unwrap();
    f.service.reconcile().await.unwrap();
    assert!(f.slots.holds("/home/.claude").is_none());
}

#[tokio::test]
async fn a_removed_account_loses_the_theme_in_its_folder() {
    let f = fixture().await;
    let work = add_account(&f.store, "work", "/accounts/work", 1).await;
    f.themes
        .install(package("night-blue", true), true, true)
        .await
        .unwrap();
    f.themes.select("night-blue", true).await.unwrap();
    f.service.set_enabled(true).await.unwrap();
    assert!(f.slots.holds("/accounts/work").is_some());

    AccountRepository::delete(
        f.store.as_ref(),
        work.id(),
        work.version(),
        Timestamp::from_millis(2),
    )
    .await
    .unwrap();
    f.service.reconcile().await.unwrap();

    assert!(f.slots.holds("/accounts/work").is_none());
    assert!(f.slots.holds("/home/.claude").is_some());
    assert_eq!(f.store.targets().await.unwrap(), ["/home/.claude"]);
}

#[tokio::test]
async fn a_foreign_folder_is_reported_and_the_others_still_get_the_theme() {
    let f = fixture().await;
    add_account(&f.store, "work", "/accounts/work", 1).await;
    f.slots
        .foreign
        .lock()
        .unwrap()
        .insert(String::from("/accounts/work"));
    f.themes
        .install(package("night-blue", true), true, true)
        .await
        .unwrap();
    f.themes.select("night-blue", true).await.unwrap();

    let status = f.service.set_enabled(true).await.unwrap();

    assert_eq!(status.state, TargetState::Foreign);
    assert!(f.slots.holds("/home/.claude").is_some());
    assert!(f.slots.holds("/accounts/work").is_none());
    let work = status
        .targets
        .iter()
        .find(|t| t.path == "/accounts/work")
        .unwrap();
    assert!(
        work.message
            .as_deref()
            .unwrap()
            .contains("not owned by NULLRADIX")
    );
    // switched off, a foreign folder is not an error and stays untouched
    let status = f.service.set_enabled(false).await.unwrap();
    assert_eq!(status.state, TargetState::None);
}

#[tokio::test]
async fn a_failing_folder_shows_as_error_and_is_kept_for_removal() {
    let f = fixture().await;
    add_account(&f.store, "work", "/accounts/work", 1).await;
    f.slots
        .broken
        .lock()
        .unwrap()
        .insert(String::from("/accounts/work"));
    f.themes
        .install(package("night-blue", true), true, true)
        .await
        .unwrap();
    f.themes.select("night-blue", true).await.unwrap();

    let status = f.service.set_enabled(true).await.unwrap();
    assert_eq!(status.state, TargetState::Error);
    assert!(f.slots.holds("/home/.claude").is_some());
    assert_eq!(
        f.store.targets().await.unwrap(),
        ["/accounts/work", "/home/.claude"]
    );
    let seen = f.service.status().await.unwrap();
    assert_eq!(seen.state, TargetState::Error);
    assert_eq!(seen.targets[1].message.as_deref(), Some("disk full"));
}

#[tokio::test]
async fn the_status_only_reads() {
    let f = fixture().await;
    f.themes
        .install(package("night-blue", true), true, true)
        .await
        .unwrap();
    f.themes.select("night-blue", true).await.unwrap();
    let before = feed(&f.store).await.len();
    f.store
        .set_enabled(true, Timestamp::from_millis(1))
        .await
        .unwrap();
    let seen = f.service.status().await.unwrap();
    assert!(seen.enabled);
    assert_eq!(seen.state, TargetState::Error, "not put there yet");
    assert_eq!(f.slots.writes(), 0);
    assert_eq!(feed(&f.store).await.len(), before + 1);
    assert_eq!(f.store.targets().await.unwrap(), Vec::<String>::new());
}

#[tokio::test]
async fn a_theme_without_a_claude_part_wants_nothing_even_when_switched_on() {
    let f = fixture().await;
    f.themes
        .install(package("plain", false), false, true)
        .await
        .unwrap();
    f.themes.select("plain", true).await.unwrap();
    let status = f.service.set_enabled(true).await.unwrap();
    assert!(status.enabled);
    assert_eq!(status.state, TargetState::None);
    assert_eq!(status.theme_id.as_deref(), Some("plain"));
    assert_eq!(f.slots.writes(), 0);
}

#[tokio::test]
async fn account_folders_spelled_alike_are_written_once() {
    let f = fixture().await;
    add_account(&f.store, "work", "/HOME/.claude", 1).await;
    f.themes
        .install(package("night-blue", true), true, true)
        .await
        .unwrap();
    f.themes.select("night-blue", true).await.unwrap();
    let status = f.service.set_enabled(true).await.unwrap();
    assert_eq!(status.targets.len(), 1);
}

#[tokio::test]
async fn setting_the_same_switch_again_records_no_change() {
    let f = fixture().await;
    f.service.set_enabled(true).await.unwrap();
    let first = feed(&f.store).await.len();
    f.service.set_enabled(true).await.unwrap();
    assert_eq!(feed(&f.store).await.len(), first);
}
