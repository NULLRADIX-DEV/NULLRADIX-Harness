//! Operating system integration for Windows and Linux.

mod browser;
mod files;
mod paths;
mod process;
#[cfg(windows)]
mod windows;

pub use browser::open_url;
pub use files::write_atomic;
pub use paths::{Paths, PathsError};
pub use process::{ProcessTree, hide_console, process_alive, spawn_detached};
