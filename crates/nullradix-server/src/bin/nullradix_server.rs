//! Entwicklungs-Binary des Rust-Servers: startet ihn mit `--port` und `--web-dir`.
//!
//! Später startet die Tauri-App denselben Server im eigenen Prozess; dieses Binary gibt es,
//! damit der Vertrag und die Handarbeit ihn ohne die App starten können:
//!
//! ```sh
//! cargo run -p nullradix-server -- --port 4710
//! ```

use std::process::ExitCode;

use nullradix_platform::Platform;
use nullradix_server::config::ServerConfigLoader;

#[tokio::main]
async fn main() -> ExitCode {
    // Wie im Binary `nullradix`: Worker und andere Kindprozesse erben die Standard-Handles nur
    // ausdrücklich
    nullradix_platform::stdio::disable_inheritance();
    nullradix_core::registry::adopt_legacy_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut config = match ServerConfigLoader::load_from_process(&args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    // Ohne Angabe liegt die gebaute Oberfläche im Repo neben dem Serverpaket
    if config.web_dir.is_none() {
        config.web_dir = Some(repo_web_dir());
    }
    let board = nullradix_server::board_from_config(config.home.clone());
    let platform = platform(&config);
    let server = match nullradix_server::serve(config, board, platform).await {
        Ok(server) => server,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "NULLRADIX-Server auf http://127.0.0.1:{} (Benutzerordner {})",
        server.port,
        server.state.home.dir().display()
    );
    nullradix_server::shutdown_signal().await;
    server.stop().await;
    ExitCode::SUCCESS
}

/// `<Repo>/packages/web/dist`, aus dem Ordner dieses Crates heraus gerechnet
fn repo_web_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("packages")
        .join("web")
        .join("dist")
}

/// Systemfunktionen zu diesen Einstellungen: im Vertragsmodus die des Vertrags, sonst die
/// dieses Rechners
fn platform(config: &nullradix_server::config::ServerConfig) -> Platform {
    match config.is_contract() {
        true => Platform::contract(),
        false => Platform::local(),
    }
}
