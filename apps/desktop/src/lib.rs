//! The desktop shell: starts or finds the daemon, signs its window in and stays in the tray.

mod launch;

use std::thread;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

use crate::launch::{LaunchError, Setup};

const MAIN: &str = "main";
/// Passed by the autostart entry, so a login does not open the window.
const HIDDEN: &str = "--hidden";

pub fn run() -> Result<(), tauri::Error> {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| show(app)))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN]),
        ))
        .setup(|app| {
            let hidden = std::env::args().any(|arg| arg == HIDDEN);
            WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
                .title("NULLRADIX")
                .inner_size(1280.0, 840.0)
                .min_inner_size(720.0, 480.0)
                .visible(!hidden)
                .build()?;
            tray(app)?;
            let handle = app.handle().clone();
            thread::spawn(move || sign_in(&handle));
            Ok(())
        })
        .on_window_event(|window, event| {
            // closing hides to the tray; the daemon and its runs go on either way
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
}

/// Points the window at the daemon's web UI, or at the local page with the reason it cannot.
fn sign_in(app: &AppHandle) {
    let target = Setup::from_env()
        .map_err(LaunchError::from)
        .and_then(|setup| launch::sign_in_url(&setup));
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    let url = match target {
        Ok(url) => url,
        Err(error) => match window.url() {
            Ok(mut page) => {
                page.query_pairs_mut()
                    .clear()
                    .append_pair("error", &error.to_string());
                page
            }
            Err(_) => return,
        },
    };
    let _ = window.navigate(url);
}

fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn tray(app: &App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open NULLRADIX", true, None::<&str>)?;
    let enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start at login",
        true,
        enabled,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &autostart, &separator, &quit])?;
    let mut builder = TrayIconBuilder::with_id(MAIN)
        .tooltip("NULLRADIX")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show(app),
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
