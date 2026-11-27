//! Operating system integration for Windows, Linux and macOS.

mod archive;
mod autostart;
mod browser;
#[cfg(target_os = "macos")]
mod claude_keychain;
mod claude_slot;
mod command;
mod files;
mod local_time;
mod paths;
mod process;
mod project_settings;
mod secrets;
mod toast;
#[cfg(windows)]
mod windows;

pub use archive::{
    ArchiveError, Entry, Limits, list_files, pack_zip, read_tree, read_within, remove_dir,
    replace_dir, unpack_zip,
};
pub use autostart::{
    AutostartEntry, AutostartState, autostart_disable, autostart_enable, autostart_reenable,
    autostart_retarget, autostart_state,
};
pub use browser::open_url;
#[cfg(target_os = "macos")]
pub use claude_keychain::claude_keychain_password;
pub use claude_slot::{
    Applied, SLOT_NAME, SlotError, SlotState, SlotTheme, SlotValue, apply as apply_claude_slot,
    clear as clear_claude_slot, inspect as inspect_claude_slot,
    settings_hash as claude_slot_settings_hash,
};
pub use files::{create_private_dir, write_atomic, write_private};
pub use local_time::{LocalZone, local_utc_offset_minutes};
pub use paths::{Paths, PathsError, claude_config_dirs, home_dir};
pub use process::{ProcessTree, process_alive, spawn_detached};
pub use project_settings::{LOCAL_SETTINGS, SettingsError, add_git_excludes, enable_plugin};
pub use secrets::{FileSecrets, SecretBackend, SecretError, system_secrets};
pub use toast::show_toast;
