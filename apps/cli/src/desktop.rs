//! What the daemon does on the person's desktop: toasts for notices and the login entry.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use nullradix_app::autostart::Autostart;
use nullradix_app::notifier::{Notice, NoticeKind, Notifier};
use nullradix_app::{AppError, ErrorKind};
use nullradix_os::{
    AutostartEntry, autostart_disable, autostart_enable, autostart_enabled, show_toast,
};
use tokio::task;

use crate::config::Profile;

/// The desktop shell, which a release install puts next to the daemon.
const SHELL: &str = "nullradix-desktop";
/// Passed to the shell so a login does not open its window.
const HIDDEN: &str = "--hidden";

/// Shows each notice as a desktop toast; the shell shows none, so a notice appears once.
pub(crate) struct ToastNotifier;

#[async_trait]
impl Notifier for ToastNotifier {
    async fn notify(&self, notice: &Notice) -> Result<(), AppError> {
        let (title, body) = toast_text(notice);
        task::spawn_blocking(move || show_toast(&title, &body))
            .await
            .map_err(AppError::internal)?
            .map_err(|error| {
                AppError::new(
                    ErrorKind::Unavailable,
                    "toast-failed",
                    format!("cannot show the toast: {error}"),
                )
            })
    }
}

const fn label(kind: NoticeKind) -> &'static str {
    match kind {
        NoticeKind::Merged => "Merged",
        NoticeKind::Blocked => "Blocked",
        NoticeKind::Question => "Question waiting",
        NoticeKind::Paused => "Dispatcher paused",
        NoticeKind::Review => "Ready for review",
        NoticeKind::Stuck => "Autopilot stuck",
        NoticeKind::MergeWaiting => "Merge waiting",
    }
}

/// Title and body of the toast: the kind and ticket, then what happened.
fn toast_text(notice: &Notice) -> (String, String) {
    let title = match &notice.ticket {
        Some(ticket) => format!("{}: {ticket}", label(notice.kind)),
        None => label(notice.kind).to_owned(),
    };
    let body = match (notice.title.trim(), notice.text.trim()) {
        ("", text) => text.to_owned(),
        (name, "") => name.to_owned(),
        (name, text) => format!("{name} - {text}"),
    };
    (title, body)
}

/// The login entry: the desktop shell when installed, else the daemon alone.
pub(crate) struct LoginEntry {
    profile: Profile,
    daemon: PathBuf,
}

impl LoginEntry {
    pub(crate) fn new(profile: Profile) -> Option<Self> {
        Some(Self {
            profile,
            daemon: env::current_exe().ok()?,
        })
    }

    fn entry(&self) -> AutostartEntry {
        entry_for(self.profile, &self.daemon, Path::is_file)
    }
}

/// Builds the entry; `exists` tells whether the shell binary is there.
fn entry_for(profile: Profile, daemon: &Path, exists: impl Fn(&Path) -> bool) -> AutostartEntry {
    let shell = daemon.with_file_name(format!("{SHELL}{}", env::consts::EXE_SUFFIX));
    let profile_args = ["--profile", profile.flag()].map(OsString::from);
    let (program, args) = if exists(&shell) {
        let mut args = vec![OsString::from(HIDDEN)];
        args.extend(profile_args);
        (shell, args)
    } else {
        let mut args = profile_args.to_vec();
        args.push(OsString::from("serve"));
        (daemon.to_path_buf(), args)
    };
    AutostartEntry {
        name: profile.app_name().to_owned(),
        label: String::from(match profile {
            Profile::Release => "NULLRADIX",
            Profile::Dev => "NULLRADIX (dev)",
        }),
        program,
        args,
    }
}

#[async_trait]
impl Autostart for LoginEntry {
    fn available(&self) -> bool {
        cfg!(any(windows, target_os = "linux"))
    }

    async fn is_enabled(&self) -> Result<bool, String> {
        let name = self.profile.app_name();
        blocking(move || autostart_enabled(name)).await
    }

    async fn set(&self, enabled: bool) -> Result<(), String> {
        let entry = self.entry();
        blocking(move || {
            if enabled {
                autostart_enable(&entry)
            } else {
                autostart_disable(&entry.name)
            }
        })
        .await
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, String> {
    task::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(kind: NoticeKind, ticket: Option<&str>, title: &str, text: &str) -> Notice {
        Notice {
            kind,
            ticket: ticket.map(str::to_owned),
            title: title.to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_toast_names_the_kind_the_ticket_and_what_happened() {
        let (title, body) = toast_text(&notice(
            NoticeKind::Merged,
            Some("ARC-1"),
            "Add search",
            "merged into main (abc123)",
        ));
        assert_eq!(title, "Merged: ARC-1");
        assert_eq!(body, "Add search - merged into main (abc123)");
        let (title, body) = toast_text(&notice(NoticeKind::Paused, None, "", "limit reached"));
        assert_eq!(
            (title.as_str(), body.as_str()),
            ("Dispatcher paused", "limit reached")
        );
        let (_, body) = toast_text(&notice(NoticeKind::Review, Some("A-2"), "Only title", " "));
        assert_eq!(body, "Only title");
    }

    #[test]
    fn the_entry_starts_the_shell_hidden_when_it_is_installed() {
        let daemon = Path::new("/opt/nullradix/nullradix");
        let entry = entry_for(Profile::Release, daemon, |_| true);
        assert_eq!(entry.name, "nullradix");
        assert!(
            entry
                .program
                .to_string_lossy()
                .contains(&format!("{SHELL}{}", env::consts::EXE_SUFFIX))
        );
        assert_eq!(
            entry.args,
            ["--hidden", "--profile", "release"].map(OsString::from)
        );
    }

    #[test]
    fn without_the_shell_the_entry_starts_the_daemon() {
        let daemon = Path::new("/opt/nullradix/nullradix");
        let entry = entry_for(Profile::Dev, daemon, |_| false);
        assert_eq!(entry.name, "nullradix-dev");
        assert_eq!(entry.program, daemon);
        assert_eq!(
            entry.args,
            ["--profile", "dev", "serve"].map(OsString::from)
        );
    }
}
