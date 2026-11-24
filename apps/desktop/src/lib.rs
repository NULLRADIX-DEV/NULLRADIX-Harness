//! The desktop shell: starts or finds the daemon, signs its window in and stays in the tray.

mod launch;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::{env, fs, thread};

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use url::Url;

use crate::launch::{LaunchError, Setup};

const MAIN: &str = "main";
/// Passed by the autostart entry, so a login does not open the window.
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
    let setup = Setup::from_env().ok();
    let mut builder = tauri::Builder::default();
    if INSTALLED {
        let mut args = vec![String::from(HIDDEN)];
        if let Some(setup) = &setup {
            args.extend(
                setup
                    .data_args()
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned()),
            );
        }
        builder = builder
            .plugin(tauri_plugin_single_instance::init(|app, _, _| show(app)))
            .plugin(tauri_plugin_autostart::Builder::new().args(args).build());
    }
    let window_shell = Arc::clone(&shell);
    builder
        .manage(Arc::clone(&shell))
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
                Ok(()) => window_shell.has_tray.store(true, Ordering::SeqCst),
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
    let target = Setup::from_env().and_then(|setup| launch::sign_in_url(&setup));
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

/// Brings the window up and signs in again when it is not showing the daemon.
fn show(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    if window.url().is_ok_and(|url| is_local(&url)) {
        reconnect(app);
    }
}

fn tray(app: &App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open NULLRADIX", true, None::<&str>)?;
    let again = MenuItem::with_id(app, "reconnect", "Reconnect", true, None::<&str>)?;
    let enabled = INSTALLED && app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start at login",
        INSTALLED,
        enabled,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &again, &autostart, &separator, &quit])?;
    let mut builder = TrayIconBuilder::with_id(MAIN)
        .tooltip("NULLRADIX")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show(app),
            "reconnect" => reconnect(app),
            "autostart" => toggle_autostart(app, &autostart),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// Flips autostart and shows what the system now says, which may differ on failure.
fn toggle_autostart(app: &AppHandle, item: &CheckMenuItem<tauri::Wry>) {
    let manager = app.autolaunch();
    let enabled = manager.is_enabled().unwrap_or(false);
    let _ = if enabled {
        manager.disable()
    } else {
        manager.enable()
    };
    let _ = item.set_checked(manager.is_enabled().unwrap_or(enabled));
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
