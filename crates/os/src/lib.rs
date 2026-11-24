//! Operating system integration for Windows and Linux.

mod paths;
mod process;
#[cfg(windows)]
mod windows;

pub use paths::{Paths, PathsError};
pub use process::process_alive;
