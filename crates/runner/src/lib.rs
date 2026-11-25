//! Executes runs on this machine under detached supervisors.

mod cleanup;
mod git;
mod local;
mod protocol;
mod supervisor;
mod transcript;
mod worktree;

pub use local::{LocalRunner, SupervisorCommand};
pub use supervisor::supervise;
