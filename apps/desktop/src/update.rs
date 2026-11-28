//! The tray item that looks for an update and installs one on a click, never on its own.

use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use tauri::menu::MenuItem;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::launch::{self, LaunchError, Setup};

/// What the item shows and does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateState {
    /// A development build never looks for updates.
    Off,
    Idle,
    Checking,
    UpToDate,
    Found(String),
    Installing(String),
    /// Runs still hold the daemon program, which the installer must replace.
    Waiting {
        version: String,
        runs: usize,
    },
    Failed,
}

/// What a click starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Check,
    Install,
}

/// How a look at the server ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Checked {
    Newer(String),
    Current,
    Failed,
}

impl UpdateState {
    pub(crate) const fn initial(installed: bool) -> Self {
        if installed { Self::Idle } else { Self::Off }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::Off => String::from("Updates: release builds only"),
            Self::Idle => String::from("Check for updates"),
            Self::Checking => String::from("Checking for updates..."),
            Self::UpToDate => String::from("Up to date (check again)"),
            Self::Found(version) => format!("Install update {version}"),
            Self::Installing(version) => format!("Installing update {version}..."),
            Self::Waiting { version, runs } => format!(
                "Install update {version} ({runs} {} still running)",
                if *runs == 1 { "run" } else { "runs" }
            ),
            Self::Failed => String::from("Update failed (check again)"),
        }
    }

    /// What a click does here, and the state it moves to while that works.
    pub(crate) fn press(&self) -> Option<(Action, Self)> {
        match self {
            Self::Idle | Self::UpToDate | Self::Failed => Some((Action::Check, Self::Checking)),
            Self::Found(version) | Self::Waiting { version, .. } => {
                Some((Action::Install, Self::Installing(version.clone())))
            }
            Self::Off | Self::Checking | Self::Installing(_) => None,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.press().is_some()
    }
}

/// Only a check the person asked for says that nothing was found or that it failed.
pub(crate) fn after_check(checked: Checked, asked: bool) -> UpdateState {
    match checked {
        Checked::Newer(version) => UpdateState::Found(version),
        Checked::Current if asked => UpdateState::UpToDate,
        Checked::Failed if asked => UpdateState::Failed,
        Checked::Current | Checked::Failed => UpdateState::Idle,
    }
}

/// Runs lock the daemon program only on Windows; elsewhere an installer replaces it freely.
pub(crate) const fn blocking_runs(active: usize, windows: bool) -> usize {
    if windows { active } else { 0 }
}

struct Inner {
    state: UpdateState,
    found: Option<Update>,
}

/// The state of the item and the update it found.
pub(crate) struct Updates {
    inner: Mutex<Inner>,
}

impl Updates {
    pub(crate) fn new(installed: bool) -> Self {
        Self {
            inner: Mutex::new(Inner {
                state: UpdateState::initial(installed),
                found: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn state(&self) -> UpdateState {
        self.lock().state.clone()
    }

    /// Starts what a click means in the current state; two clicks never start two jobs.
    fn press(&self, only: Option<Action>) -> Option<(Action, Option<Update>)> {
        let mut inner = self.lock();
        let (action, next) = inner.state.press()?;
        if only.is_some_and(|only| only != action) {
            return None;
        }
        if action == Action::Install && inner.found.is_none() {
            inner.state = UpdateState::Idle;
            return None;
        }
        inner.state = next;
        Some((action, inner.found.clone()))
    }

    fn set(&self, state: UpdateState, found: Option<Update>) {
        let mut inner = self.lock();
        inner.state = state;
        inner.found = found;
    }
}

fn render(item: &MenuItem<Wry>, state: &UpdateState) {
    if let Err(error) = item
        .set_text(state.label())
        .and_then(|()| item.set_enabled(state.enabled()))
    {
        eprintln!("cannot update the tray: {error}");
    }
}

/// Looks once, when the app starts, without a word unless there is something to install.
pub(crate) fn check_at_start(app: &AppHandle, item: &MenuItem<Wry>) {
    run(app, item, Some(Action::Check));
}

/// The click on the item: looks for an update or installs the one found.
pub(crate) fn click(app: &AppHandle, item: &MenuItem<Wry>) {
    run(app, item, None);
}

/// `only` limits what may start; the start of the app never installs.
fn run(app: &AppHandle, item: &MenuItem<Wry>, only: Option<Action>) {
    let updates = Arc::clone(app.state::<Arc<Updates>>().inner());
    let Some((action, found)) = updates.press(only) else {
        return;
    };
    let asked = only.is_none();
    render(item, &updates.state());
    let (app, item) = (app.clone(), item.clone());
    thread::spawn(move || match action {
        Action::Check => check(&app, &item, &updates, asked),
        Action::Install => {
            if let Some(update) = found {
                install(&app, &item, &updates, &update);
            }
        }
    });
}

fn check(app: &AppHandle, item: &MenuItem<Wry>, updates: &Updates, asked: bool) {
    let looked = app
        .updater()
        .and_then(|updater| tauri::async_runtime::block_on(updater.check()));
    let (checked, found) = match looked {
        Ok(Some(update)) => (Checked::Newer(update.version.clone()), Some(update)),
        Ok(None) => (Checked::Current, None),
        Err(error) => {
            eprintln!("cannot check for updates: {error}");
            (Checked::Failed, None)
        }
    };
    let state = after_check(checked, asked);
    updates.set(state.clone(), found);
    render(item, &state);
}

#[derive(Debug, thiserror::Error)]
enum InstallError {
    #[error(transparent)]
    Launch(#[from] LaunchError),
    #[error("cannot download the update: {0}")]
    Download(tauri_plugin_updater::Error),
    #[error("the installer failed: {0}")]
    Install(tauri_plugin_updater::Error),
}

fn install(app: &AppHandle, item: &MenuItem<Wry>, updates: &Updates, update: &Update) {
    let held = Setup::from_env()
        .and_then(|setup| launch::active_runs(&setup))
        .map(|active| blocking_runs(active, cfg!(windows)));
    match held {
        Ok(0) => {}
        Ok(runs) => {
            let waiting = UpdateState::Waiting {
                version: update.version.clone(),
                runs,
            };
            updates.set(waiting.clone(), Some(update.clone()));
            render(item, &waiting);
            return;
        }
        Err(error) => {
            give_up(item, updates, &InstallError::from(error));
            return;
        }
    }
    if let Err(error) = replace(app, update) {
        give_up(item, updates, &error);
        // after the download the daemon may be down; this starts it again
        if !matches!(error, InstallError::Download(_)) {
            crate::reconnect(app);
        }
    }
}

fn give_up(item: &MenuItem<Wry>, updates: &Updates, error: &InstallError) {
    eprintln!("cannot install the update: {error}");
    updates.set(UpdateState::Failed, None);
    render(item, &UpdateState::Failed);
}

/// Downloads first, so the daemon is down only while the installer runs.
fn replace(app: &AppHandle, update: &Update) -> Result<(), InstallError> {
    let setup = Setup::from_env()?;
    let bytes = tauri::async_runtime::block_on(update.download(|_, _| {}, || {}))
        .map_err(InstallError::Download)?;
    launch::stop_daemon(&setup)?;
    // on Windows the installer ends this process and starts the new app itself
    update.install(&bytes).map_err(InstallError::Install)?;
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_development_build_never_checks() {
        assert_eq!(UpdateState::initial(false), UpdateState::Off);
        assert_eq!(UpdateState::initial(true), UpdateState::Idle);
        assert_eq!(UpdateState::Off.press(), None);
        assert!(!UpdateState::Off.enabled());
    }

    #[test]
    fn the_labels_say_what_a_click_does() {
        let found = UpdateState::Found(String::from("0.2.0"));
        assert_eq!(UpdateState::Idle.label(), "Check for updates");
        assert_eq!(found.label(), "Install update 0.2.0");
        assert_eq!(
            UpdateState::Installing(String::from("0.2.0")).label(),
            "Installing update 0.2.0..."
        );
        let waiting = |runs| UpdateState::Waiting {
            version: String::from("0.2.0"),
            runs,
        };
        assert_eq!(
            waiting(1).label(),
            "Install update 0.2.0 (1 run still running)"
        );
        assert_eq!(
            waiting(3).label(),
            "Install update 0.2.0 (3 runs still running)"
        );
        for state in [
            UpdateState::Off,
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::UpToDate,
            found,
            waiting(2),
            UpdateState::Failed,
        ] {
            assert!(state.label().is_ascii(), "{state:?}");
        }
    }

    #[test]
    fn a_click_checks_or_installs_but_never_twice_at_once() {
        for state in [
            UpdateState::Idle,
            UpdateState::UpToDate,
            UpdateState::Failed,
        ] {
            assert_eq!(state.press(), Some((Action::Check, UpdateState::Checking)));
        }
        let installing = UpdateState::Installing(String::from("0.2.0"));
        for state in [
            UpdateState::Found(String::from("0.2.0")),
            UpdateState::Waiting {
                version: String::from("0.2.0"),
                runs: 2,
            },
        ] {
            assert_eq!(state.press(), Some((Action::Install, installing.clone())));
        }
        assert_eq!(UpdateState::Checking.press(), None);
        assert_eq!(installing.press(), None);
        assert!(!installing.enabled());
    }

    #[test]
    fn only_a_newer_version_shows_without_a_click() {
        let newer = || Checked::Newer(String::from("0.2.0"));
        let found = UpdateState::Found(String::from("0.2.0"));
        assert_eq!(after_check(newer(), false), found);
        assert_eq!(after_check(newer(), true), found);
        assert_eq!(after_check(Checked::Current, false), UpdateState::Idle);
        assert_eq!(after_check(Checked::Failed, false), UpdateState::Idle);
        assert_eq!(after_check(Checked::Current, true), UpdateState::UpToDate);
        assert_eq!(after_check(Checked::Failed, true), UpdateState::Failed);
    }

    #[test]
    fn active_runs_block_the_installer_only_on_windows() {
        assert_eq!(blocking_runs(2, true), 2);
        assert_eq!(blocking_runs(2, false), 0);
        assert_eq!(blocking_runs(0, true), 0);
    }

    #[test]
    fn the_item_starts_one_job_and_keeps_what_it_found() {
        let updates = Updates::new(true);
        assert!(matches!(updates.press(None), Some((Action::Check, None))));
        assert_eq!(updates.state(), UpdateState::Checking);
        assert!(updates.press(None).is_none());
        updates.set(UpdateState::Found(String::from("0.2.0")), None);
        // found without an update to install falls back instead of installing nothing
        assert!(updates.press(None).is_none());
        assert_eq!(updates.state(), UpdateState::Idle);
        assert!(Updates::new(false).press(None).is_none());
    }

    #[test]
    fn the_start_of_the_app_may_check_but_not_install() {
        let updates = Updates::new(true);
        updates.set(UpdateState::Failed, None);
        assert!(updates.press(Some(Action::Install)).is_none());
        assert_eq!(updates.state(), UpdateState::Failed);
        assert!(updates.press(Some(Action::Check)).is_some());
        assert_eq!(updates.state(), UpdateState::Checking);
        let waiting = Updates::new(true);
        waiting.set(UpdateState::Found(String::from("0.2.0")), None);
        assert!(waiting.press(Some(Action::Check)).is_none());
        assert_eq!(waiting.state(), UpdateState::Found(String::from("0.2.0")));
    }
}
