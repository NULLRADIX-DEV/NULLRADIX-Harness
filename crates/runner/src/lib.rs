//! Executes runs on this machine under detached supervisors.

mod accounts;
mod claude_login;
mod cleanup;
mod curl;
mod ensure;
mod git;
// only Windows starts through it; its rules are tested everywhere
#[cfg_attr(not(windows), allow(dead_code))]
mod launch;
mod local;
mod login_process;
mod oauth_refresh;
mod protocol;
mod supervisor;
mod transcript;
mod usage_api;
mod worktree;

pub use accounts::LocalAccountFolders;
pub use claude_login::LocalClaudeLogins;
pub use local::{LocalRunner, SupervisorCommand};
pub use login_process::LocalLoginRunner;
pub use oauth_refresh::{
    CLIENT_ID, CurlTokenEndpoint, LocalLoginRenewal, TOKEN_URL, TokenEndpoint, TokenReply,
};
pub use supervisor::supervise;
pub use transcript::TranscriptRoots;
pub use usage_api::{CurlUsageApi, USAGE_URL};
