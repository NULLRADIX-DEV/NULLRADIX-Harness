// Unter Windows startet die Release-App ohne Konsolenfenster.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    nullradix_core::registry::adopt_legacy_env();
    // `dispatch-worker <lauf>` kommt vom Server im Prozess der App; dieser Start arbeitet nur den
    // Lauf ab und baut keine Oberfläche auf (kein Dock-Symbol, kein Fenster).
    if nullradix_desktop_lib::run_dispatch_worker() {
        return;
    }
    nullradix_desktop_lib::run()
}
