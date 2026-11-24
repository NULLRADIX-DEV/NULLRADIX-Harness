//! Operating system integration for Windows and Linux.

mod browser;
mod paths;
mod process;
#[cfg(windows)]
mod windows;

pub use browser::open_url;
pub use paths::{Paths, PathsError};
pub use process::process_alive;
