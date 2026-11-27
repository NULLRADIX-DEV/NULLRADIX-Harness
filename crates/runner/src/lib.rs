//! Executes runs on this machine under detached supervisors.

mod accounts;
mod catalog_fetch;
mod claude_login;
mod cleanup;
mod curl;
mod detached;
mod ensure;
mod git;
// only Windows starts through it; its rules are tested everywhere
#[cfg_attr(not(windows), allow(dead_code))]
mod launch;
mod local;
mod login_process;
mod merge;
mod oauth_refresh;
mod plugin;
mod protocol;
mod review;
mod setup;
mod supervisor;
mod transcript;
mod usage_api;
mod worktree;

pub use accounts::{LocalAccountFolders, expand_home};
pub use catalog_fetch::CurlCatalogFetcher;
pub use claude_login::LocalClaudeLogins;
pub use detached::LocalDetached;
pub use local::{LocalRunner, SupervisorCommand};
pub use login_process::LocalLoginRunner;
pub use merge::LocalMergeGit;
pub use oauth_refresh::{
    CLIENT_ID, CurlTokenEndpoint, LocalLoginRenewal, TOKEN_URL, TokenEndpoint, TokenReply,
};
pub use setup::LocalSetup;
pub use supervisor::supervise;
pub use transcript::TranscriptRoots;
pub use usage_api::{CurlUsageApi, USAGE_URL};
