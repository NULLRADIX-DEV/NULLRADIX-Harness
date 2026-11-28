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
    /// A process holds the daemon program that the installer must replace.
    Waiting {
        version: String,
    },
    Failed(Reason),
}

/// Where a failed update stopped; the details go to the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    Check,
    Setup,
    Download,
    Daemon,
    Installer,
}

impl Reason {
    const fn word(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Setup => "setup",
            Self::Download => "download",
            Self::Daemon => "daemon did not stop",
            Self::Installer => "installer",
        }
    }
}

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
            Self::Waiting { version } => {
                format!("Install update {version} (daemon program still in use)")
            }
            Self::Failed(reason) => format!("Update failed: {} (check again)", reason.word()),
        }
    }

    /// What a click does here, and the state it moves to while that works.
    pub(crate) fn press(&self) -> Option<(Action, Self)> {
        match self {
            Self::Idle | Self::UpToDate | Self::Failed(_) => Some((Action::Check, Self::Checking)),
            Self::Found(version) | Self::Waiting { version } => {
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
        Checked::Failed if asked => UpdateState::Failed(Reason::Check),
        Checked::Current | Checked::Failed => UpdateState::Idle,
    }
}

struct Inner {
    state: UpdateState,
    found: Option<Update>,
}

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

    /// An install is under way and nothing may start the daemon.
    pub(crate) fn installing(&self) -> bool {
        matches!(self.lock().state, UpdateState::Installing(_))
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
            log(&UpdateError::Check(error), asked);
            (Checked::Failed, None)
        }
    };
    let state = after_check(checked, asked);
    updates.set(state.clone(), found);
    render(item, &state);
}

#[derive(Debug, thiserror::Error)]
enum UpdateError {
    #[error("cannot check for updates: {0}")]
    Check(tauri_plugin_updater::Error),
    #[error("cannot start the update: {0}")]
    Setup(LaunchError),
    #[error("cannot download the update: {0}")]
    Download(tauri_plugin_updater::Error),
    #[error("cannot stop the daemon for the update: {0}")]
    Daemon(LaunchError),
    #[error("the installer failed: {0}")]
    Installer(tauri_plugin_updater::Error),
}

impl UpdateError {
    const fn reason(&self) -> Reason {
        match self {
            Self::Check(_) => Reason::Check,
            Self::Setup(_) => Reason::Setup,
            Self::Download(_) => Reason::Download,
            Self::Daemon(_) => Reason::Daemon,
            Self::Installer(_) => Reason::Installer,
        }
    }

    /// From the stop on the daemon may be down and has to be started again.
    const fn daemon_may_be_down(&self) -> bool {
        matches!(self, Self::Daemon(_) | Self::Installer(_))
    }
}

/// What kept an install from finishing.
enum Blocked {
    /// A process still holds the daemon program, and only waiting helps.
    Held,
    Failed(UpdateError),
}

impl From<UpdateError> for Blocked {
    fn from(error: UpdateError) -> Self {
        Self::Failed(error)
    }
}

impl Blocked {
    /// The state to show and whether the update stays for another click.
    fn outcome(&self, version: &str) -> (UpdateState, bool) {
        match self {
            Self::Held => (
                UpdateState::Waiting {
                    version: version.to_owned(),
                },
                true,
            ),
            Self::Failed(error) => (UpdateState::Failed(error.reason()), false),
        }
    }

    const fn daemon_may_be_down(&self) -> bool {
        match self {
            Self::Held => true,
            Self::Failed(error) => error.daemon_may_be_down(),
        }
    }
}

/// Release builds have no console; failures the person asked about go to desktop.log.
fn log(error: &UpdateError, asked: bool) {
    if asked {
        crate::report_failure(error);
    } else {
        eprintln!("{error}");
    }
}

fn install(app: &AppHandle, item: &MenuItem<Wry>, updates: &Updates, update: &Update) {
    launch::hold_daemon_down(true);
    let Err(blocked) = replace(app, update) else {
        return;
    };
    launch::hold_daemon_down(false);
    if let Blocked::Failed(error) = &blocked {
        log(error, true);
    }
    let (state, keep) = blocked.outcome(&update.version);
    updates.set(state.clone(), keep.then(|| update.clone()));
    render(item, &state);
    if blocked.daemon_may_be_down() {
        crate::reconnect(app);
    }
}

/// Downloads first; the daemon is down only from the stop until the installer has run.
fn replace(app: &AppHandle, update: &Update) -> Result<(), Blocked> {
    let setup = Setup::from_env().map_err(UpdateError::Setup)?;
    let bytes = tauri::async_runtime::block_on(update.download(|_, _| {}, || {}))
        .map_err(UpdateError::Download)?;
    launch::stop_daemon(&setup).map_err(UpdateError::Daemon)?;
    // runs and CLI calls outlive the daemon and may still hold its program
    if !launch::program_released(&setup.daemon) {
        return Err(Blocked::Held);
    }
    // on Windows the installer ends this process and starts the new app itself
    update.install(&bytes).map_err(UpdateError::Installer)?;
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version() -> String {
        String::from("0.2.0")
    }

    fn waiting() -> UpdateState {
        UpdateState::Waiting { version: version() }
    }

    #[test]
    fn a_development_build_never_checks() {
        assert_eq!(UpdateState::initial(false), UpdateState::Off);
        assert_eq!(UpdateState::initial(true), UpdateState::Idle);
        assert_eq!(UpdateState::Off.press(), None);
        assert!(!UpdateState::Off.enabled());
    }

    #[test]
    fn the_labels_say_what_a_click_does() {
        let found = UpdateState::Found(version());
        assert_eq!(UpdateState::Idle.label(), "Check for updates");
        assert_eq!(found.label(), "Install update 0.2.0");
        assert_eq!(
            UpdateState::Installing(version()).label(),
            "Installing update 0.2.0..."
        );
        assert_eq!(
            waiting().label(),
            "Install update 0.2.0 (daemon program still in use)"
        );
        assert_eq!(
            UpdateState::Failed(Reason::Download).label(),
            "Update failed: download (check again)"
        );
        for state in [
            UpdateState::Off,
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::UpToDate,
            found,
            waiting(),
            UpdateState::Failed(Reason::Daemon),
        ] {
            assert!(state.label().is_ascii(), "{state:?}");
        }
    }

    #[test]
    fn a_click_checks_or_installs_but_never_twice_at_once() {
        for state in [
            UpdateState::Idle,
            UpdateState::UpToDate,
            UpdateState::Failed(Reason::Installer),
        ] {
            assert_eq!(state.press(), Some((Action::Check, UpdateState::Checking)));
        }
        let installing = UpdateState::Installing(version());
        for state in [UpdateState::Found(version()), waiting()] {
            assert_eq!(state.press(), Some((Action::Install, installing.clone())));
        }
        assert_eq!(UpdateState::Checking.press(), None);
        assert_eq!(installing.press(), None);
        assert!(!installing.enabled());
    }

    #[test]
    fn only_a_newer_version_shows_without_a_click() {
        let newer = || Checked::Newer(version());
        let found = UpdateState::Found(version());
        assert_eq!(after_check(newer(), false), found);
        assert_eq!(after_check(newer(), true), found);
        assert_eq!(after_check(Checked::Current, false), UpdateState::Idle);
        assert_eq!(after_check(Checked::Failed, false), UpdateState::Idle);
        assert_eq!(after_check(Checked::Current, true), UpdateState::UpToDate);
        assert_eq!(
            after_check(Checked::Failed, true),
            UpdateState::Failed(Reason::Check)
        );
    }

    #[test]
    fn a_failure_names_where_it_stopped_and_whether_the_daemon_may_be_down() {
        let launch = || LaunchError::StopTimeout;
        let installer = || tauri_plugin_updater::Error::EmptyEndpoints;
        let cases = [
            (UpdateError::Setup(launch()), Reason::Setup, false),
            (UpdateError::Download(installer()), Reason::Download, false),
            (UpdateError::Daemon(launch()), Reason::Daemon, true),
            (UpdateError::Installer(installer()), Reason::Installer, true),
        ];
        for (error, reason, down) in cases {
            assert_eq!(error.reason(), reason, "{error}");
            assert_eq!(error.daemon_may_be_down(), down, "{error}");
        }
    }

    #[test]
    fn a_held_program_waits_with_the_update_and_a_failure_drops_it() {
        let (state, keep) = Blocked::Held.outcome("0.2.0");
        assert_eq!(state, waiting());
        assert!(keep && Blocked::Held.daemon_may_be_down());
        let failed = Blocked::from(UpdateError::Download(
            tauri_plugin_updater::Error::EmptyEndpoints,
        ));
        let (state, keep) = failed.outcome("0.2.0");
        assert_eq!(state, UpdateState::Failed(Reason::Download));
        assert!(!keep && !failed.daemon_may_be_down());
    }

    #[test]
    fn the_item_starts_one_job_and_keeps_what_it_found() {
        let updates = Updates::new(true);
        assert!(matches!(updates.press(None), Some((Action::Check, None))));
        assert_eq!(updates.state(), UpdateState::Checking);
        assert!(updates.press(None).is_none());
        updates.set(UpdateState::Found(version()), None);
        // found without an update to install falls back instead of installing nothing
        assert!(updates.press(None).is_none());
        assert_eq!(updates.state(), UpdateState::Idle);
        assert!(Updates::new(false).press(None).is_none());
    }

    #[test]
    fn the_start_of_the_app_may_check_but_not_install() {
        let updates = Updates::new(true);
        updates.set(UpdateState::Failed(Reason::Check), None);
        assert!(updates.press(Some(Action::Install)).is_none());
        assert_eq!(updates.state(), UpdateState::Failed(Reason::Check));
        assert!(updates.press(Some(Action::Check)).is_some());
        assert_eq!(updates.state(), UpdateState::Checking);
        let found = Updates::new(true);
        found.set(UpdateState::Found(version()), None);
        assert!(found.press(Some(Action::Check)).is_none());
        assert_eq!(found.state(), UpdateState::Found(version()));
    }

    #[test]
    fn only_an_install_under_way_holds_the_daemon_down() {
        let updates = Updates::new(true);
        assert!(!updates.installing());
        for state in [
            UpdateState::Checking,
            UpdateState::Found(version()),
            waiting(),
            UpdateState::Failed(Reason::Daemon),
        ] {
            updates.set(state, None);
            assert!(!updates.installing());
        }
        updates.set(UpdateState::Installing(version()), None);
        assert!(updates.installing());
    }
}
