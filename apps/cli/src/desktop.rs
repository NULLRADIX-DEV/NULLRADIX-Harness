//! What the daemon does on the person's desktop: toasts for notices and the login entry.

use std::env;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::autostart::Autostart;
use nullradix_app::notifier::{Notice, NoticeKind, Notifier, toast_failed};
use nullradix_os::{
    AutostartEntry, autostart_disable, autostart_enable, autostart_reenable, autostart_retarget,
    autostart_state, show_toast,
};
use tokio::sync::Semaphore;
use tokio::task;

use crate::config::Profile;

/// The desktop shell, which a release install puts next to the daemon.
const SHELL: &str = "nullradix-desktop";
/// Passed to the shell so a login does not open its window.
const HIDDEN: &str = "--hidden";
/// Toasts that run or wait at once; a notice beyond that is dropped.
const IN_FLIGHT: usize = 4;

type Show = dyn Fn(&str, &str) -> io::Result<()> + Send + Sync;

/// Shows each notice as a desktop toast in the background; the shell shows none.
pub(crate) struct ToastNotifier {
    show: Arc<Show>,
    slots: Arc<Semaphore>,
    missing_logged: Arc<AtomicBool>,
}

impl ToastNotifier {
    pub(crate) fn new() -> Self {
        Self::with(Arc::new(show_toast))
    }

    fn with(show: Arc<Show>) -> Self {
        Self {
            show,
            slots: Arc::new(Semaphore::new(IN_FLIGHT)),
            missing_logged: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// A missing helper is told once, any other failure every time.
fn loud(error: &io::Error, missing_logged: &AtomicBool) -> bool {
    error.kind() != io::ErrorKind::NotFound || !missing_logged.swap(true, Ordering::Relaxed)
}

#[async_trait]
impl Notifier for ToastNotifier {
    async fn notify(&self, notice: &Notice) -> Result<(), AppError> {
        let Ok(permit) = Arc::clone(&self.slots).try_acquire_owned() else {
            tracing::debug!(
                kind = notice.kind.as_str(),
                "too many toasts at once, dropped one"
            );
            return Ok(());
        };
        let (title, body) = toast_text(notice);
        let show = Arc::clone(&self.show);
        let missing_logged = Arc::clone(&self.missing_logged);
        // a cold helper takes seconds, which the dispatcher must not wait for
        task::spawn_blocking(move || {
            let _permit = permit;
            if let Err(error) = show(&title, &body) {
                let failure = toast_failed(&error.to_string());
                if loud(&error, &missing_logged) {
                    tracing::warn!("{}", failure.message());
                } else {
                    tracing::debug!("{}", failure.message());
                }
            }
        });
        Ok(())
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
    appimage: Option<PathBuf>,
}

impl LoginEntry {
    pub(crate) fn new(profile: Profile) -> Option<Self> {
        Some(Self {
            profile,
            daemon: env::current_exe().ok()?,
            appimage: env::var_os("APPIMAGE")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
        })
    }

    fn entry(&self) -> AutostartEntry {
        entry_for(
            self.profile,
            &self.daemon,
            self.appimage.as_deref(),
            Path::is_file,
        )
    }

    /// A build output or a mounted image is no place to start from; an install is.
    fn installed(&self) -> bool {
        self.appimage.is_some() || !is_build_output(&self.daemon)
    }
}

/// Builds the entry; `exists` tells whether the shell binary is there.
fn entry_for(
    profile: Profile,
    daemon: &Path,
    appimage: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> AutostartEntry {
    let shell = daemon.with_file_name(format!("{SHELL}{}", env::consts::EXE_SUFFIX));
    let profile_args = ["--profile", profile.flag()].map(OsString::from);
    let hidden = |program: PathBuf| {
        let mut args = vec![OsString::from(HIDDEN)];
        args.extend(profile_args.clone());
        (program, args)
    };
    // the daemon of an image runs from a mount that is gone after the next start
    let (program, args) = if let Some(image) = appimage {
        hidden(image.to_path_buf())
    } else if exists(&shell) {
        hidden(shell)
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

/// Cargo output or a mounted `AppImage`: a path that does not survive.
fn is_build_output(path: &Path) -> bool {
    path.components().any(|part| {
        let part = part.as_os_str().to_string_lossy();
        part == "target" || part.starts_with(".mount_")
    })
}

/// Whether the entry should be pointed at the running program: it is gone or this is the install.
fn retarget_wanted(
    current: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
    installed: bool,
) -> bool {
    installed || !current.is_some_and(exists)
}

#[async_trait]
impl Autostart for LoginEntry {
    fn available(&self) -> bool {
        cfg!(any(windows, target_os = "linux"))
    }

    async fn is_enabled(&self) -> Result<bool, String> {
        let name = self.profile.app_name();
        blocking(move || Ok(autostart_state(name)?.enabled())).await
    }

    async fn set(&self, enabled: bool) -> Result<(), String> {
        let entry = self.entry();
        let installed = self.installed();
        blocking(move || {
            if !enabled {
                return autostart_disable(&entry.name);
            }
            let state = autostart_state(&entry.name)?;
            // a build must not take the entry over from a program that still exists
            if state.exists && !retarget_wanted(state.program.as_deref(), Path::is_file, installed)
            {
                autostart_reenable(&entry.name)
            } else {
                autostart_enable(&entry)
            }
        })
        .await
    }

    async fn refresh(&self) -> Result<(), String> {
        let entry = self.entry();
        let installed = self.installed();
        blocking(move || {
            let state = autostart_state(&entry.name)?;
            if state.exists && retarget_wanted(state.program.as_deref(), Path::is_file, installed) {
                autostart_retarget(&entry)?;
            }
            Ok(())
        })
        .await
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> Result<T, String> {
    task::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::time::Duration;

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

    async fn until(condition: impl Fn() -> bool) {
        for _ in 0..2000 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the condition never held");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_slow_toast_never_holds_up_the_caller_and_the_queue_is_bounded() {
        let (release, gate) = mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(gate));
        let started = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicUsize::new(0));
        let show: Arc<Show> = {
            let (gate, started, finished) = (gate.clone(), started.clone(), finished.clone());
            Arc::new(move |_, _| {
                started.fetch_add(1, Ordering::SeqCst);
                // blocks until the test lets go, which a caller that waited would never survive
                let _ = gate.lock().unwrap().recv();
                finished.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        };
        let notifier = ToastNotifier::with(show);
        let one = notice(NoticeKind::Merged, Some("A-1"), "t", "x");
        for _ in 0..IN_FLIGHT + 3 {
            notifier.notify(&one).await.unwrap();
        }
        until(|| started.load(Ordering::SeqCst) == IN_FLIGHT).await;
        for _ in 0..IN_FLIGHT + 3 {
            let _ = release.send(());
        }
        until(|| finished.load(Ordering::SeqCst) == IN_FLIGHT).await;
        assert_eq!(started.load(Ordering::SeqCst), IN_FLIGHT);
        // the slots are free again afterwards
        notifier.notify(&one).await.unwrap();
        until(|| started.load(Ordering::SeqCst) == IN_FLIGHT + 1).await;
    }

    #[test]
    fn a_missing_helper_is_told_once_and_other_failures_every_time() {
        let missing = io::Error::from(io::ErrorKind::NotFound);
        let logged = AtomicBool::new(false);
        assert!(loud(&missing, &logged));
        assert!(!loud(&missing, &logged));
        let other = io::Error::other("boom");
        assert!(loud(&other, &logged));
        assert!(loud(&other, &logged));
    }

    #[test]
    fn the_entry_starts_the_shell_hidden_when_it_is_installed() {
        let daemon = Path::new("/opt/nullradix/nullradix");
        let entry = entry_for(Profile::Release, daemon, None, |_| true);
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
        let entry = entry_for(Profile::Dev, daemon, None, |_| false);
        assert_eq!(entry.name, "nullradix-dev");
        assert_eq!(entry.program, daemon);
        assert_eq!(
            entry.args,
            ["--profile", "dev", "serve"].map(OsString::from)
        );
    }

    #[test]
    fn an_appimage_starts_itself_not_the_daemon_inside_its_mount() {
        let daemon = Path::new("/tmp/.mount_nullraXYZ/usr/bin/nullradix");
        let image = Path::new("/home/ada/Apps/NULLRADIX.AppImage");
        let entry = entry_for(Profile::Release, daemon, Some(image), |_| true);
        assert_eq!(entry.program, image);
        assert_eq!(entry.args[0], OsString::from("--hidden"));
    }

    #[test]
    fn builds_and_mounts_are_no_install() {
        assert!(is_build_output(Path::new(
            "/src/harness/target/debug/nullradix"
        )));
        assert!(is_build_output(Path::new(
            "/tmp/.mount_nullraXYZ/usr/bin/nullradix"
        )));
        assert!(!is_build_output(Path::new("/usr/bin/nullradix")));
        assert!(!is_build_output(Path::new(
            "/home/ada/.cargo/bin/nullradix"
        )));
    }

    #[test]
    fn a_build_only_takes_the_entry_over_when_its_program_is_gone() {
        let exists = |path: &Path| path == Path::new("/usr/bin/nullradix-desktop");
        let current = Some(Path::new("/usr/bin/nullradix-desktop"));
        // installed binary: always point the entry at it
        assert!(retarget_wanted(current, exists, true));
        // a build keeps an entry whose program still exists
        assert!(!retarget_wanted(current, exists, false));
        // and replaces one that is gone or missing
        assert!(retarget_wanted(Some(Path::new("/gone")), exists, false));
        assert!(retarget_wanted(None, exists, false));
    }
}
