//! The desktop shell: starts or finds the daemon, signs its window in and stays in the tray.

mod launch;
mod tray;
mod update;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::{env, fs, thread};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use url::Url;

use crate::launch::{LaunchError, Setup};

const MAIN: &str = "main";
/// Passed by the login entry the daemon writes, so a login does not open the window.
const HIDDEN: &str = "--hidden";
/// Release builds only, so a development shell never takes over the installed one.
const INSTALLED: bool = !cfg!(debug_assertions);

/// What the shell knows while it runs.
#[derive(Debug, Default)]
struct Shell {
    /// The daemon the window is signed in to; the only remote origin it may show.
    daemon: Mutex<Option<Url>>,
    signing_in: AtomicBool,
    has_tray: AtomicBool,
}

impl Shell {
    fn daemon(&self) -> Option<Url> {
        self.daemon
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_daemon(&self, origin: Option<Url>) {
        *self.daemon.lock().unwrap_or_else(PoisonError::into_inner) = origin;
    }
}

pub fn run() -> Result<(), tauri::Error> {
    let shell = Arc::new(Shell::default());
    let mut builder = tauri::Builder::default();
    if INSTALLED {
        builder = builder
            .plugin(tauri_plugin_single_instance::init(|app, _, _| show(app)))
            .plugin(tauri_plugin_updater::Builder::new().build());
    }
    let window_shell = Arc::clone(&shell);
    builder
        .manage(Arc::clone(&shell))
        .manage(Arc::new(update::Updates::new(INSTALLED)))
        .setup(move |app| {
            let hidden = env::args_os().any(|arg| arg == HIDDEN);
            let guard = Arc::clone(&window_shell);
            WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
                .title("NULLRADIX")
                .inner_size(1280.0, 840.0)
                .min_inner_size(720.0, 480.0)
                .visible(!hidden)
                .on_navigation(move |url| allow(&guard, url))
                .build()?;
            match tray(app) {
                Ok((rows, update_item)) => {
                    window_shell.has_tray.store(true, Ordering::SeqCst);
                    tray::watch(app.handle(), rows);
                    update::check_at_start(app.handle(), &update_item);
                }
                Err(error) => eprintln!("no tray icon, closing the window quits: {error}"),
            }
            reconnect(app.handle());
            Ok(())
        })
        .on_window_event(move |window, event| {
            // with a tray, closing hides; the daemon and its runs go on either way
            if let WindowEvent::CloseRequested { api, .. } = event
                && shell.has_tray.load(Ordering::SeqCst)
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
}

fn is_local(url: &Url) -> bool {
    url.scheme() == "tauri" || url.host_str() == Some("tauri.localhost")
}

/// The local page and the signed-in daemon stay in the window; other links open in the browser.
fn allow(shell: &Shell, url: &Url) -> bool {
    let daemon = shell.daemon();
    if is_local(url) || daemon.is_some_and(|daemon| daemon.origin() == url.origin()) {
        return true;
    }
    if matches!(url.scheme(), "http" | "https") {
        let _ = nullradix_os::open_url(url.as_str());
    }
    false
}

/// The bundled page that shows progress, or a failure passed as `?error=`.
fn local_page(error: Option<&str>) -> Option<Url> {
    let base = if cfg!(windows) {
        "http://tauri.localhost/index.html"
    } else {
        "tauri://localhost/index.html"
    };
    let mut page = Url::parse(base).ok()?;
    if let Some(error) = error {
        page.query_pairs_mut().append_pair("error", error);
    }
    Some(page)
}

/// Signs the window in again in the background, unless that is already under way.
fn reconnect(app: &AppHandle) {
    // a sign-in now would start the daemon again under the installer
    if app.state::<Arc<update::Updates>>().installing() {
        return;
    }
    let shell = app.state::<Arc<Shell>>().inner().clone();
    if shell.signing_in.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    thread::spawn(move || {
        sign_in(&app, &shell);
        shell.signing_in.store(false, Ordering::SeqCst);
    });
}

/// Points the window at the daemon's web UI, or at the local page with the reason it cannot.
fn sign_in(app: &AppHandle, shell: &Shell) {
    let resources = app.path().resource_dir().ok();
    let target = Setup::from_env()
        .map(|setup| setup.with_resources(resources.as_deref()))
        .and_then(|setup| launch::sign_in_url(&setup));
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    let url = match target {
        Ok(url) => {
            let origin = Url::parse(&url.origin().ascii_serialization()).ok();
            shell.set_daemon(origin);
            Some(url)
        }
        Err(error) => {
            shell.set_daemon(None);
            local_page(Some(&error.to_string()))
        }
    };
    if let Some(url) = url
        && let Err(error) = window.navigate(url)
    {
        eprintln!("cannot show the page: {error}");
    }
}

/// Whether the window may be handed a confirmation code.
#[derive(Debug, PartialEq, Eq)]
enum ConfirmTarget {
    /// The window shows the daemon it was signed in to.
    Deliver,
    /// The window shows the local page or another origin.
    NotSignedIn,
}

/// Only a window still showing the signed-in daemon gets a code, never another page.
fn confirm_target(shown: Option<&Url>, daemon: Option<&Url>) -> ConfirmTarget {
    match (shown, daemon) {
        (Some(shown), Some(daemon)) if !is_local(shown) && shown.origin() == daemon.origin() => {
            ConfirmTarget::Deliver
        }
        _ => ConfirmTarget::NotSignedIn,
    }
}

const NOT_SIGNED_IN: &str =
    "This window is not signed in. Choose Reconnect in the tray menu, then confirm again.";

/// Brings the window to the front, also from the tray or when it is minimized.
fn raise(window: &tauri::WebviewWindow) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// Says on the local page what is wrong: the window is not signed in.
fn report_not_signed_in(window: &tauri::WebviewWindow) {
    raise(window);
    if let Some(page) = local_page(Some(NOT_SIGNED_IN))
        && let Err(error) = window.navigate(page)
    {
        eprintln!("cannot show the page: {error}");
    }
}

/// Hands the signed-in window a code in its fragment: no other page can read it.
fn confirm_changes(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    let shell = app.state::<Arc<Shell>>().inner().clone();
    let target = |window: &tauri::WebviewWindow| {
        confirm_target(window.url().ok().as_ref(), shell.daemon().as_ref())
    };
    if target(&window) == ConfirmTarget::NotSignedIn {
        report_not_signed_in(&window);
        return;
    }
    let resources = app.path().resource_dir().ok();
    let code = Setup::from_env()
        .map(|setup| setup.with_resources(resources.as_deref()))
        .and_then(|setup| launch::step_up_code(&setup));
    match code {
        // the window may have moved on while the code was made
        Ok(_) if target(&window) == ConfirmTarget::NotSignedIn => report_not_signed_in(&window),
        // hex needs no quoting in a script string
        Ok(code) => {
            if let Err(error) = window.eval(format!("window.location.hash = 'confirm={code}'")) {
                eprintln!("cannot confirm: {error}");
            }
            raise(&window);
        }
        Err(error) => eprintln!("cannot confirm: {error}"),
    }
}

/// Brings the window up and signs in again when it is not showing the daemon.
fn show(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    raise(&window);
    if window.url().is_ok_and(|url| is_local(&url)) {
        reconnect(app);
    }
}

fn tray(app: &App) -> tauri::Result<(tray::Rows<tauri::Wry>, MenuItem<tauri::Wry>)> {
    let initial = tray::texts(&tray::Snapshot::default());
    let open = MenuItem::with_id(app, "open", "Open NULLRADIX", true, None::<&str>)?;
    let again = MenuItem::with_id(app, "reconnect", "Reconnect", true, None::<&str>)?;
    let confirm = MenuItem::with_id(app, "confirm", "Confirm changes", true, None::<&str>)?;
    let update_state = app.state::<Arc<update::Updates>>().state();
    let update_item = MenuItem::with_id(
        app,
        "update",
        update_state.label(),
        update_state.enabled(),
        None::<&str>,
    )?;
    // the status lines only show; they cannot be clicked
    let dispatcher = MenuItem::with_id(
        app,
        "status-dispatcher",
        &initial.dispatcher,
        false,
        None::<&str>,
    )?;
    let inbox = MenuItem::with_id(app, "status-inbox", &initial.inbox, false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &again,
            &confirm,
            &update_item,
            &PredefinedMenuItem::separator(app)?,
            &dispatcher,
            &inbox,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let clicked = update_item.clone();
    let mut builder = TrayIconBuilder::with_id(tray::TRAY_ID)
        .tooltip(&initial.tooltip)
        .menu(&menu)
        // on macOS a left click opens the menu, so the window needs no click of its own there
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show(app),
            "reconnect" => reconnect(app),
            "confirm" => {
                let app = app.clone();
                thread::spawn(move || confirm_changes(&app));
            }
            "update" => update::click(app, &clicked),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
                && !cfg!(target_os = "macos")
            {
                show(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok((tray::Rows { dispatcher, inbox }, update_item))
}

/// Leaves a failure where a windowed build, which has no console, can show it later.
pub fn report_failure(error: &dyn std::error::Error) {
    eprintln!("error: {error}");
    let logs = Setup::from_env()
        .and_then(|setup| setup.paths().map_err(LaunchError::from))
        .map(|paths| paths.logs);
    if let Ok(logs) = logs
        && fs::create_dir_all(&logs).is_ok()
    {
        let _ = fs::write(logs.join("desktop.log"), format!("{error}\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_local_page_and_the_signed_in_daemon_stay_in_the_window() {
        let shell = Shell::default();
        let page = local_page(None).unwrap();
        assert!(allow(&shell, &page));
        let daemon = Url::parse("http://127.0.0.1:4711/").unwrap();
        let board = daemon.join("/board").unwrap();
        assert!(!allow(
            &shell,
            &Url::parse("ftp://127.0.0.1:4711/board").unwrap()
        ));
        shell.set_daemon(Some(daemon));
        assert!(allow(&shell, &board));
        assert!(!allow(
            &shell,
            &Url::parse("ftp://127.0.0.1:4712/").unwrap()
        ));
    }

    #[test]
    fn a_code_goes_only_to_a_window_that_shows_the_signed_in_daemon() {
        let daemon = Url::parse("http://127.0.0.1:4711/").unwrap();
        let board = daemon.join("/board").unwrap();
        let other_port = Url::parse("http://127.0.0.1:4712/board").unwrap();
        let elsewhere = Url::parse("https://example.com/").unwrap();
        let local = local_page(None).unwrap();
        let deliver = |shown: &Url| confirm_target(Some(shown), Some(&daemon));
        assert_eq!(deliver(&board), ConfirmTarget::Deliver);
        assert_eq!(deliver(&other_port), ConfirmTarget::NotSignedIn);
        assert_eq!(deliver(&elsewhere), ConfirmTarget::NotSignedIn);
        assert_eq!(deliver(&local), ConfirmTarget::NotSignedIn);
        assert_eq!(
            confirm_target(Some(&board), None),
            ConfirmTarget::NotSignedIn
        );
        assert_eq!(
            confirm_target(None, Some(&daemon)),
            ConfirmTarget::NotSignedIn
        );
    }

    fn config(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn the_updater_config_is_one_the_plugin_accepts() {
        let base = config(include_str!("../tauri.conf.json"));
        let updater: tauri_plugin_updater::Config =
            serde_json::from_value(base["plugins"]["updater"].clone()).unwrap();
        assert_eq!(updater.endpoints.len(), 1);
        assert!(updater.endpoints.iter().all(|url| url.scheme() == "https"));
        assert!(updater.pubkey.len() > 100);
        assert!(
            updater
                .windows
                .is_some_and(|windows| windows.install_mode.to_string() == "passive")
        );
    }

    #[test]
    fn only_release_builds_make_signed_updater_files() {
        let release = config(include_str!("../tauri.release.conf.json"));
        assert_eq!(release["bundle"]["createUpdaterArtifacts"], true);
        // a development build has no signing key
        let base = config(include_str!("../tauri.conf.json"));
        assert!(base["bundle"].get("createUpdaterArtifacts").is_none());
    }

    #[test]
    fn failures_travel_to_the_local_page_encoded() {
        let page = local_page(Some("no daemon & no luck")).unwrap();
        assert!(is_local(&page));
        let error = page
            .query_pairs()
            .find(|(key, _)| key == "error")
            .map(|(_, value)| value.into_owned());
        assert_eq!(error.as_deref(), Some("no daemon & no luck"));
    }
}
