//! The Claude config folders of this machine: where a theme's Claude files go.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use nullradix_app::claude_theme::{ClaudeSlots, SlotApplied, SlotFailure, SlotState, SlotTheme};
use nullradix_domain::{CLAUDE_DIR, SettingValue};
use nullradix_os::{SlotError, SlotValue};

/// Claude files of installed themes: the folder per theme id, and the default Claude folder.
pub(crate) struct FolderSlots {
    themes: PathBuf,
    default_dir: Option<PathBuf>,
}

impl FolderSlots {
    pub(crate) const fn new(themes: PathBuf, default_dir: Option<PathBuf>) -> Self {
        Self {
            themes,
            default_dir,
        }
    }
}

fn failure(error: SlotError) -> SlotFailure {
    match error {
        SlotError::Foreign(path) => SlotFailure::Foreign(path.display().to_string()),
        other => SlotFailure::Failed(other.to_string()),
    }
}

fn slot_values(settings: &BTreeMap<String, SettingValue>) -> BTreeMap<String, SlotValue> {
    settings
        .iter()
        .map(|(key, value)| {
            let value = match value {
                SettingValue::Switch(on) => SlotValue::Switch(*on),
                SettingValue::Choice(text) => SlotValue::Choice(text.clone()),
            };
            (key.clone(), value)
        })
        .collect()
}

async fn blocking<T, F>(work: F) -> Result<T, SlotFailure>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, SlotFailure> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| SlotFailure::Failed(error.to_string()))?
}

#[async_trait]
impl ClaudeSlots for FolderSlots {
    fn default_dir(&self) -> Option<String> {
        self.default_dir
            .as_deref()
            .map(|dir| dir.display().to_string())
    }

    fn key(&self, dir: &str) -> String {
        if cfg!(any(windows, target_os = "macos")) {
            dir.to_lowercase()
        } else {
            dir.to_owned()
        }
    }

    fn settings_hash(&self, theme: &SlotTheme) -> String {
        nullradix_os::claude_slot_settings_hash(&slot_values(&theme.settings))
    }

    async fn inspect(&self, dir: &str) -> Result<SlotState, SlotFailure> {
        let dir = PathBuf::from(dir);
        blocking(move || {
            Ok(
                match nullradix_os::inspect_claude_slot(&dir).map_err(failure)? {
                    nullradix_os::SlotState::Empty => SlotState::Empty,
                    nullradix_os::SlotState::Managed {
                        theme_id,
                        theme_version,
                        settings_hash,
                    } => SlotState::Managed {
                        theme_id,
                        theme_version,
                        settings_hash,
                    },
                    nullradix_os::SlotState::Foreign => SlotState::Foreign,
                },
            )
        })
        .await
    }

    async fn apply(&self, dir: &str, theme: &SlotTheme) -> Result<SlotApplied, SlotFailure> {
        let dir = PathBuf::from(dir);
        let source = self.themes.join(&theme.id).join(CLAUDE_DIR);
        let theme = theme.clone();
        blocking(move || {
            let settings = slot_values(&theme.settings);
            let input = nullradix_os::SlotTheme {
                id: &theme.id,
                version: &theme.version,
                name: &theme.name,
                author: &theme.author,
                settings: &settings,
            };
            let applied =
                nullradix_os::apply_claude_slot(&dir, &source, &input, crate::themes::LIMITS)
                    .map_err(failure)?;
            Ok(SlotApplied {
                written: applied.written,
                removed: applied.removed,
            })
        })
        .await
    }

    async fn clear(&self, dir: &str) -> Result<bool, SlotFailure> {
        let dir = PathBuf::from(dir);
        blocking(move || nullradix_os::clear_claude_slot(&dir).map_err(failure)).await
    }
}

/// The folder Claude reads without an account: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub(crate) fn default_claude_dir(isolated_home: Option<&Path>) -> Option<PathBuf> {
    match isolated_home {
        Some(home) => Some(home.join(".claude")),
        None => nullradix_os::claude_config_dirs().into_iter().next(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use nullradix_app::accounts::AccountRepository;
    use nullradix_app::claude_theme::{ClaudeThemeService, TargetState};
    use nullradix_app::themes::ThemeService;
    use nullradix_app::{ChangeSignal, SystemClock, UuidV7Generator};
    use nullradix_domain::{Account, AccountName, AccountSlug, ConfigDir, Id, Timestamp};
    use nullradix_os::Entry;
    use nullradix_store::Store;

    use super::*;
    use crate::themes::FolderThemes;

    const MANIFEST: &str = r#"{
        "id": "night", "name": "Night", "version": "1.0.0", "author": "Ada",
        "sdkVersion": "^1.3", "colorScheme": "dark", "slots": [], "claudeTheme": true,
        "settingsSchema": { "dim": { "type": "boolean", "label": "Dim", "default": true } }
    }"#;

    fn entry(path: &str, data: &str) -> Entry {
        Entry {
            path: path.to_owned(),
            data: data.as_bytes().to_vec(),
        }
    }

    fn package() -> Vec<u8> {
        nullradix_os::pack_zip(&[
            entry("manifest.json", MANIFEST),
            entry("theme.css", "a {}"),
            entry("claude/hooks/hooks.json", r#"{"modules":["./look.ts"]}"#),
            entry("claude/hooks/look.ts", "export {}"),
        ])
        .unwrap()
    }

    struct Setup {
        _dir: tempfile::TempDir,
        root: PathBuf,
        store: Arc<Store>,
        themes: Arc<ThemeService>,
        service: ClaudeThemeService,
    }

    async fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let store = Arc::new(Store::in_memory().await.unwrap());
        let signal = ChangeSignal::new();
        let themes_dir = root.join("themes");
        let themes = Arc::new(ThemeService::new(
            store.clone(),
            Arc::new(FolderThemes::new(themes_dir.clone())),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let service = ClaudeThemeService::new(
            store.clone(),
            themes.clone(),
            store.clone(),
            Arc::new(FolderSlots::new(themes_dir, Some(root.join("default")))),
            Arc::new(SystemClock),
            signal,
        );
        Setup {
            _dir: dir,
            root,
            store,
            themes,
            service,
        }
    }

    #[tokio::test]
    async fn the_switch_writes_real_files_into_every_folder_and_off_removes_them() {
        let s = setup().await;
        let work = s.root.join("work");
        fs::create_dir_all(&work).unwrap();
        let account = Account::new(
            Id::from_uuid(uuid::Uuid::now_v7()),
            AccountSlug::parse("work").unwrap(),
            AccountName::parse("Work").unwrap(),
            ConfigDir::parse(&work.display().to_string()).unwrap(),
            1,
            Timestamp::from_millis(1),
        );
        AccountRepository::insert(s.store.as_ref(), &account)
            .await
            .unwrap();
        fs::create_dir_all(s.root.join("default")).unwrap();
        fs::write(
            s.root.join("default/settings.json"),
            r#"{"keep":true,"hooks":{}}"#,
        )
        .unwrap();
        s.themes.install(package()).await.unwrap();
        s.themes.select("night").await.unwrap();

        // installing and choosing alone leave the Claude folders alone
        s.service.reconcile().await.unwrap();
        assert!(!s.root.join("default/skills").exists());

        let status = s.service.set_enabled(true).await.unwrap();
        assert_eq!(status.state, TargetState::Active);
        for dir in [s.root.join("default"), work.clone()] {
            let slot = dir.join("skills/nullradix-theme");
            assert_eq!(
                fs::read_to_string(slot.join("hooks/look.ts")).unwrap(),
                "export {}"
            );
            let settings = fs::read_to_string(slot.join("hooks/nullradix-settings.ts")).unwrap();
            assert!(settings.contains(r#"{"dim":true}"#), "{settings}");
        }
        assert_eq!(
            fs::read_to_string(s.root.join("default/settings.json")).unwrap(),
            r#"{"keep":true,"hooks":{}}"#
        );

        s.service.set_enabled(false).await.unwrap();
        assert!(!s.root.join("default/skills/nullradix-theme").exists());
        assert!(!work.join("skills/nullradix-theme").exists());
        assert!(work.is_dir());
    }

    #[tokio::test]
    async fn a_theme_folder_that_vanished_is_an_error_not_a_panic() {
        let s = setup().await;
        s.themes.install(package()).await.unwrap();
        s.themes.select("night").await.unwrap();
        fs::remove_dir_all(s.root.join("themes/night/claude")).unwrap();
        let status = s.service.set_enabled(true).await.unwrap();
        assert_eq!(status.state, TargetState::Error);
        assert!(!s.root.join("default/skills").exists());
    }

    #[tokio::test]
    async fn a_legacy_slot_goes_away_at_startup_while_the_switch_is_off() {
        let s = setup().await;
        let slot = s.root.join("default/skills/nullradix-theme");
        fs::create_dir_all(slot.join("hooks")).unwrap();
        fs::write(slot.join("hooks/hooks.json"), "{}").unwrap();
        fs::write(slot.join("plugin.json"), "{}").unwrap();
        fs::write(
            slot.join(".nullradix-theme.json"),
            r#"{"managedBy":"nullradix","themeId":"night","themeVersion":"1.0.0","settings":{}}"#,
        )
        .unwrap();
        let status = s.service.reconcile().await.unwrap();
        assert!(!status.enabled);
        assert!(!slot.exists());
        assert!(s.root.join("default/skills").is_dir());
    }

    #[test]
    fn the_isolated_home_gets_its_own_default_folder() {
        let home = Path::new("/iso");
        assert_eq!(
            default_claude_dir(Some(home)),
            Some(PathBuf::from("/iso/.claude"))
        );
    }
}
