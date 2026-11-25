//! Operating system integration for Windows, Linux and macOS.

mod browser;
mod files;
mod paths;
mod process;
#[cfg(windows)]
mod windows;

pub use browser::open_url;
pub use files::write_atomic;
pub use paths::{Paths, PathsError, claude_config_dirs};
pub use process::{ProcessTree, process_alive, spawn_detached};
