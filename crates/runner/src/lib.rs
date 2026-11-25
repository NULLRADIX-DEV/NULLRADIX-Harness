//! Executes runs on this machine under detached supervisors.

mod accounts;
mod cleanup;
mod git;
mod local;
mod protocol;
mod supervisor;
mod transcript;
mod worktree;

pub use accounts::LocalAccountFolders;
pub use local::{LocalRunner, SupervisorCommand};
pub use supervisor::supervise;
