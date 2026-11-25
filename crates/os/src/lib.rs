//! Operating system integration for Windows, Linux and macOS.

mod browser;
mod files;
mod paths;
mod process;
mod secrets;
#[cfg(windows)]
mod windows;

pub use browser::open_url;
pub use files::{write_atomic, write_private};
pub use paths::{Paths, PathsError, claude_config_dirs, home_dir};
pub use process::{ProcessTree, process_alive, spawn_detached};
pub use secrets::{FileSecrets, SecretBackend, SecretError, system_secrets};
