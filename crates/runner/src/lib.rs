//! Executes runs on this machine under detached supervisors.

mod accounts;
mod claude_login;
mod cleanup;
mod git;
mod local;
mod protocol;
mod supervisor;
mod transcript;
mod usage_api;
mod worktree;

pub use accounts::LocalAccountFolders;
pub use claude_login::LocalClaudeLogins;
pub use local::{LocalRunner, SupervisorCommand};
pub use supervisor::supervise;
pub use usage_api::{CurlUsageApi, USAGE_URL};
