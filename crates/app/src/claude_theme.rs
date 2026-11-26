//! The Claude part of a theme, put into the Claude folders only while the person's switch is on.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{InstalledTheme, SettingSpec, SettingValue, Timestamp};
use parking_lot::Mutex;

use crate::accounts::AccountRepository;
use crate::themes::ThemeService;
use crate::{AppError, Change, ChangeSignal, Clock};

/// Entity id of the switch in the change feed, which only takes UUIDs.
pub const CLAUDE_THEME_FEED_ID: uuid::Uuid =
    uuid::Uuid::from_u128(0x6e75_6c6c_7261_6469_7800_636c_6175_6465);

/// The switch and the folders a theme was put into.
#[async_trait]
pub trait ClaudeThemeRepository: Send + Sync {
    /// Off until someone turned it on.
    async fn enabled(&self) -> Result<bool, AppError>;
    /// `None` when the switch stays as it is.
    async fn set_enabled(&self, enabled: bool, at: Timestamp) -> Result<Option<Change>, AppError>;
    /// Config folders that may still hold the Claude part, so a removal finds them again.
    async fn targets(&self) -> Result<Vec<String>, AppError>;
    async fn set_targets(&self, targets: &[String], at: Timestamp) -> Result<(), AppError>;
}

/// What a config folder holds at the theme's place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotState {
    Empty,
    Managed {
        theme_id: String,
        theme_version: String,
    },
    /// Not created by NULLRADIX; never written or deleted.
    Foreign,
}

/// The theme a config folder is filled with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotTheme {
    pub id: String,
    pub version: String,
    pub name: String,
    pub author: String,
    /// Values the theme runs with: saved ones, else the defaults of the manifest.
    pub settings: BTreeMap<String, SettingValue>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlotApplied {
    pub written: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SlotFailure {
    #[error("{0} is not owned by NULLRADIX; rename or delete it so the Claude theme can go there")]
    Foreign(String),
    #[error("{0}")]
    Failed(String),
}

/// The Claude config folders of this machine and what a theme's Claude files do in them.
#[async_trait]
pub trait ClaudeSlots: Send + Sync {
    /// Folder of the default Claude login, when the host has one.
    fn default_dir(&self) -> Option<String>;
    /// Comparable spelling of a folder; equal keys mean the same folder.
    fn key(&self, dir: &str) -> String;
    async fn inspect(&self, dir: &str) -> Result<SlotState, SlotFailure>;
    async fn apply(&self, dir: &str, theme: &SlotTheme) -> Result<SlotApplied, SlotFailure>;
    /// `true` when something was removed.
    async fn clear(&self, dir: &str) -> Result<bool, SlotFailure>;
}

/// For hosts without Claude folders: nothing to write.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSlots;

#[async_trait]
impl ClaudeSlots for NoSlots {
    fn default_dir(&self) -> Option<String> {
        None
    }

    fn key(&self, dir: &str) -> String {
        dir.to_owned()
    }

    async fn inspect(&self, _dir: &str) -> Result<SlotState, SlotFailure> {
        Ok(SlotState::Empty)
    }

    async fn apply(&self, dir: &str, _theme: &SlotTheme) -> Result<SlotApplied, SlotFailure> {
        Err(SlotFailure::Failed(format!(
            "cannot write to {dir} on this host"
        )))
    }

    async fn clear(&self, _dir: &str) -> Result<bool, SlotFailure> {
        Ok(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetState {
    /// The folder holds the Claude part of the chosen theme.
    Active,
    /// Nothing wanted here.
    None,
    Foreign,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetStatus {
    pub path: String,
    pub state: TargetState,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeThemeStatus {
    pub enabled: bool,
    /// Worst of the folders: error, then foreign, then active, else none.
    pub state: TargetState,
    /// The chosen theme, when it is installed.
    pub theme_id: Option<String>,
    pub theme_name: Option<String>,
    pub targets: Vec<TargetStatus>,
}

/// What the switch and the chosen theme ask for.
struct Wish {
    enabled: bool,
    chosen: Option<InstalledTheme>,
    theme: Option<SlotTheme>,
}

pub struct ClaudeThemeService {
    repository: Arc<dyn ClaudeThemeRepository>,
    themes: Arc<ThemeService>,
    accounts: Arc<dyn AccountRepository>,
    slots: Arc<dyn ClaudeSlots>,
    clock: Arc<dyn Clock>,
    signal: ChangeSignal,
    /// One reconcile at a time, so two runs never write the same folder.
    run: tokio::sync::Mutex<()>,
    /// Why the last reconcile failed, by folder.
    failures: Mutex<BTreeMap<String, String>>,
}

impl std::fmt::Debug for ClaudeThemeService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeThemeService").finish_non_exhaustive()
    }
}

impl ClaudeThemeService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn ClaudeThemeRepository>,
        themes: Arc<ThemeService>,
        accounts: Arc<dyn AccountRepository>,
        slots: Arc<dyn ClaudeSlots>,
        clock: Arc<dyn Clock>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            repository,
            themes,
            accounts,
            slots,
            clock,
            signal,
            run: tokio::sync::Mutex::new(()),
            failures: Mutex::new(BTreeMap::new()),
        }
    }

    /// Turns the switch and brings every folder in line; a failing folder shows in the status.
    pub async fn set_enabled(&self, enabled: bool) -> Result<ClaudeThemeStatus, AppError> {
        if let Some(change) = self
            .repository
            .set_enabled(enabled, self.clock.now())
            .await?
        {
            self.signal.notify(&change);
        }
        self.reconcile().await
    }

    /// What the folders hold now; reads only.
    pub async fn status(&self) -> Result<ClaudeThemeStatus, AppError> {
        let wish = self.wish().await?;
        let targets = self.targets().await?;
        let mut shown = Vec::new();
        for dir in targets {
            let status = match (&wish.theme, wish.enabled) {
                (Some(theme), true) => self.inspected(&dir, theme).await,
                _ => TargetStatus {
                    path: dir,
                    state: TargetState::None,
                    message: None,
                },
            };
            shown.push(status);
        }
        Ok(summary(&wish, shown))
    }

    /// Puts the Claude part into every folder while the switch is on, else takes it out.
    pub async fn reconcile(&self) -> Result<ClaudeThemeStatus, AppError> {
        let _turn = self.run.lock().await;
        let wish = self.wish().await?;
        let targets = self.targets().await?;
        let keys: BTreeSet<String> = targets.iter().map(|dir| self.slots.key(dir)).collect();
        let mut kept = BTreeSet::new();
        // folders that left the list (a removed account) are cleaned like the others
        for old in self.repository.targets().await? {
            if !keys.contains(&self.slots.key(&old)) {
                match self.slots.clear(&old).await {
                    Ok(_) | Err(SlotFailure::Foreign(_)) => {}
                    Err(error) => {
                        tracing::warn!(%error, folder = %old, "cannot take the Claude theme out of a folder");
                        kept.insert(old);
                    }
                }
            }
        }
        let mut shown = Vec::new();
        let mut failures = BTreeMap::new();
        for dir in targets {
            let outcome = match (&wish.theme, wish.enabled) {
                (Some(theme), true) => self.slots.apply(&dir, theme).await.map(|applied| {
                    if applied != SlotApplied::default() {
                        tracing::info!(
                            theme = %theme.id, folder = %dir,
                            written = applied.written, removed = applied.removed,
                            "put the Claude theme into a folder"
                        );
                    }
                    TargetState::Active
                }),
                _ => self.slots.clear(&dir).await.map(|removed| {
                    if removed {
                        tracing::info!(folder = %dir, "took the Claude theme out of a folder");
                    }
                    TargetState::None
                }),
            };
            let (state, message) = match outcome {
                Ok(state) => (state, None),
                // nothing wanted and the folder is not ours: nothing to do
                Err(SlotFailure::Foreign(_)) if wish.theme.is_none() || !wish.enabled => {
                    (TargetState::None, None)
                }
                Err(error @ SlotFailure::Foreign(_)) => {
                    (TargetState::Foreign, Some(error.to_string()))
                }
                Err(error) => {
                    tracing::warn!(%error, folder = %dir, "cannot sync the Claude theme");
                    (TargetState::Error, Some(error.to_string()))
                }
            };
            if let Some(text) = &message {
                failures.insert(dir.clone(), text.clone());
            }
            if state == TargetState::Active || state == TargetState::Error {
                kept.insert(dir.clone());
            }
            shown.push(TargetStatus {
                path: dir,
                state,
                message,
            });
        }
        *self.failures.lock() = failures;
        let kept: Vec<String> = kept.into_iter().collect();
        self.repository.set_targets(&kept, self.clock.now()).await?;
        Ok(summary(&wish, shown))
    }

    async fn inspected(&self, dir: &str, theme: &SlotTheme) -> TargetStatus {
        let known = self.failures.lock().get(dir).cloned();
        let (state, message) = match self.slots.inspect(dir).await {
            Ok(SlotState::Managed {
                theme_id,
                theme_version,
            }) if theme_id == theme.id && theme_version == theme.version => {
                (TargetState::Active, None)
            }
            Ok(SlotState::Foreign) => (
                TargetState::Foreign,
                Some(SlotFailure::Foreign(dir.to_owned()).to_string()),
            ),
            Ok(_) => (
                TargetState::Error,
                Some(
                    known.unwrap_or_else(|| String::from("the Claude theme is not put there yet")),
                ),
            ),
            Err(error) => (TargetState::Error, Some(error.to_string())),
        };
        TargetStatus {
            path: dir.to_owned(),
            state,
            message,
        }
    }

    /// The default folder and the folder of every account, each once.
    async fn targets(&self) -> Result<Vec<String>, AppError> {
        let mut dirs: Vec<String> = self.slots.default_dir().into_iter().collect();
        for account in self.accounts.list().await? {
            if let Some(dir) = account.config_dir() {
                dirs.push(dir.as_str().to_owned());
            }
        }
        let mut seen = BTreeSet::new();
        dirs.retain(|dir| seen.insert(self.slots.key(dir)));
        Ok(dirs)
    }

    async fn wish(&self) -> Result<Wish, AppError> {
        let enabled = self.repository.enabled().await?;
        let list = self.themes.list().await?;
        let chosen = list
            .themes
            .into_iter()
            .find(|theme| theme.id().as_str() == list.active);
        let theme = match chosen
            .as_ref()
            .filter(|theme| theme.record().manifest.record().claude_theme)
        {
            Some(theme) => {
                let saved = self.themes.settings(theme.id().as_str()).await?;
                Some(slot_theme(theme, saved.values.values()))
            }
            None => None,
        };
        Ok(Wish {
            enabled,
            chosen,
            theme,
        })
    }
}

fn summary(wish: &Wish, targets: Vec<TargetStatus>) -> ClaudeThemeStatus {
    let has = |state| targets.iter().any(|target| target.state == state);
    let state = if has(TargetState::Error) {
        TargetState::Error
    } else if has(TargetState::Foreign) {
        TargetState::Foreign
    } else if has(TargetState::Active) {
        TargetState::Active
    } else {
        TargetState::None
    };
    let manifest = wish
        .chosen
        .as_ref()
        .map(|theme| theme.record().manifest.record());
    ClaudeThemeStatus {
        enabled: wish.enabled,
        state,
        theme_id: manifest.map(|manifest| manifest.id.to_string()),
        theme_name: manifest.map(|manifest| manifest.name.clone()),
        targets,
    }
}

/// The values the theme runs with: a saved value of the right kind, else the manifest default.
fn slot_theme(theme: &InstalledTheme, saved: &BTreeMap<String, SettingValue>) -> SlotTheme {
    let manifest = theme.record().manifest.record();
    let settings = manifest
        .settings
        .iter()
        .flatten()
        .map(|(key, spec)| {
            let value = match (spec, saved.get(key)) {
                (SettingSpec::Boolean { .. }, Some(SettingValue::Switch(on))) => {
                    SettingValue::Switch(*on)
                }
                (SettingSpec::Boolean { default, .. }, _) => SettingValue::Switch(*default),
                (SettingSpec::Select { options, .. }, Some(SettingValue::Choice(text)))
                    if options.iter().any(|option| option.value == *text) =>
                {
                    SettingValue::Choice(text.clone())
                }
                (SettingSpec::Select { default, .. }, _) => SettingValue::Choice(default.clone()),
            };
            (key.clone(), value)
        })
        .collect();
    SlotTheme {
        id: manifest.id.to_string(),
        version: manifest.version.clone(),
        name: manifest.name.clone(),
        author: manifest.author.clone(),
        settings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme_package::tests::MANIFEST;
    use nullradix_domain::{ThemeSettingValues, Timestamp};

    fn installed(claude: bool) -> InstalledTheme {
        let json = MANIFEST.replace(
            r#""slots": []"#,
            &format!(r#""slots": [], "claudeTheme": {claude}"#),
        );
        InstalledTheme::install(
            crate::theme_package::parse_manifest(&json).unwrap(),
            Timestamp::from_millis(1),
        )
    }

    #[test]
    fn saved_values_win_when_they_fit_and_defaults_fill_the_rest() {
        let theme = installed(true);
        let defaults = slot_theme(&theme, ThemeSettingValues::default().values());
        assert_eq!(defaults.id, "night-blue");
        let wrong = BTreeMap::from([(String::from("nope"), SettingValue::Switch(true))]);
        assert_eq!(slot_theme(&theme, &wrong).settings, defaults.settings);
    }

    #[test]
    fn the_summary_reports_the_worst_folder() {
        let wish = Wish {
            enabled: true,
            chosen: None,
            theme: None,
        };
        let target = |state| TargetStatus {
            path: String::from("a"),
            state,
            message: None,
        };
        assert_eq!(summary(&wish, vec![]).state, TargetState::None);
        let both = summary(
            &wish,
            vec![target(TargetState::Active), target(TargetState::Foreign)],
        );
        assert_eq!(both.state, TargetState::Foreign);
        let worse = summary(
            &wish,
            vec![target(TargetState::Foreign), target(TargetState::Error)],
        );
        assert_eq!(worse.state, TargetState::Error);
    }
}
