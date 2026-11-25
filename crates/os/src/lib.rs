//! Operating system integration for Windows, Linux and macOS.

mod archive;
mod browser;
#[cfg(target_os = "macos")]
mod claude_keychain;
mod command;
mod files;
mod paths;
mod process;
mod secrets;
#[cfg(windows)]
mod windows;

pub use archive::{
    ArchiveError, Entry, Limits, pack_zip, read_tree, read_within, remove_dir, replace_dir,
    unpack_zip,
};
pub use browser::open_url;
#[cfg(target_os = "macos")]
pub use claude_keychain::claude_keychain_password;
pub use files::{create_private_dir, write_atomic, write_private};
pub use paths::{Paths, PathsError, claude_config_dirs, home_dir};
pub use process::{ProcessTree, process_alive, spawn_detached};
pub use secrets::{FileSecrets, SecretBackend, SecretError, system_secrets};
