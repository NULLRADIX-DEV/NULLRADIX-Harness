//! Tray status: three icons and two status lines, read from the daemon every few seconds.

use std::thread;
use std::time::Duration;

use nullradix_client::{Client, DiscoveryFile, token};
use tauri::image::Image;
use tauri::menu::MenuItem;
use tauri::{AppHandle, Runtime};

use crate::launch::Setup;

/// The tray icon has one id.
pub(crate) const TRAY_ID: &str = "main";
const INTERVAL: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayState {
    Ready,
    Waiting,
    Question,
}

impl TrayState {
    fn icon(self) -> tauri::Result<Image<'static>> {
        // the menu bar of macOS shrinks the icon, so it gets twice the pixels
        #[cfg(target_os = "macos")]
        let bytes: &[u8] = match self {
            Self::Ready => include_bytes!("../icons/tray/ready@2x.png"),
            Self::Waiting => include_bytes!("../icons/tray/waiting@2x.png"),
            Self::Question => include_bytes!("../icons/tray/question@2x.png"),
        };
        #[cfg(not(target_os = "macos"))]
        let bytes: &[u8] = match self {
            Self::Ready => include_bytes!("../icons/tray/ready.png"),
            Self::Waiting => include_bytes!("../icons/tray/waiting.png"),
            Self::Question => include_bytes!("../icons/tray/question.png"),
        };
        Image::from_bytes(bytes)
    }

    const fn word(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Waiting => "waiting",
            Self::Question => "question open",
        }
    }
}

/// What the daemon says about the dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DispatcherLine {
    pub(crate) enabled: bool,
    pub(crate) state: String,
    pub(crate) message: String,
}

/// What waits for the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InboxLine {
    pub(crate) questions: u32,
    pub(crate) total: u32,
}

impl InboxLine {
    const fn others(self) -> u32 {
        self.total.saturating_sub(self.questions)
    }
}

/// One look at the daemon; both parts missing means it is not reachable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) dispatcher: Option<DispatcherLine>,
    pub(crate) inbox: Option<InboxLine>,
}

/// A question comes first, then anything else that waits; a running dispatcher is not waiting.
pub(crate) fn state(snapshot: &Snapshot) -> TrayState {
    match snapshot.inbox {
        Some(inbox) if inbox.questions > 0 => TrayState::Question,
        Some(inbox) if inbox.others() > 0 => TrayState::Waiting,
        _ => TrayState::Ready,
    }
}

/// The texts of tooltip and menu; they change together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Texts {
    pub(crate) tooltip: String,
    pub(crate) dispatcher: String,
    pub(crate) inbox: String,
}

pub(crate) fn texts(snapshot: &Snapshot) -> Texts {
    let state = state(snapshot);
    let dispatcher = match (&snapshot.dispatcher, snapshot.inbox) {
        (Some(line), _) if !line.enabled => String::from("Dispatcher: off"),
        (Some(line), _) if !line.message.is_empty() => format!("Dispatcher: {}", line.message),
        (Some(line), _) => format!("Dispatcher: {}", line.state),
        (None, Some(_)) => String::from("Dispatcher: unknown"),
        (None, None) => String::from("Daemon not reachable"),
    };
    let inbox = match snapshot.inbox {
        Some(inbox) => format!(
            "Open: {} questions, {} more",
            inbox.questions,
            inbox.others()
        ),
        None => String::from("Open: unknown"),
    };
    Texts {
        tooltip: format!("NULLRADIX - {}", state.word()),
        dispatcher,
        inbox,
    }
}

/// Looks at the daemon of these settings; a daemon that is not running gives an empty snapshot.
fn fetch(setup: &Setup) -> Snapshot {
    let Ok(paths) = setup.paths() else {
        return Snapshot::default();
    };
    let Ok(Some(discovery)) = DiscoveryFile::in_dir(&paths.data).read() else {
        return Snapshot::default();
    };
    if !discovery.process_alive() {
        return Snapshot::default();
    }
    let Ok(Some(secret)) = token::read(&token::path(&paths)) else {
        return Snapshot::default();
    };
    let client = Client::new(discovery.address, CONNECT_TIMEOUT, TIMEOUT).with_token(secret);
    Snapshot {
        dispatcher: client
            .dispatcher_status()
            .ok()
            .map(|status| DispatcherLine {
                enabled: status.enabled,
                state: status.state_word().to_owned(),
                message: status.message,
            }),
        inbox: client.get_inbox(None, None).ok().map(|inbox| InboxLine {
            questions: inbox.counts.question,
            total: inbox.total,
        }),
    }
}

/// The two status lines of the tray menu; only their text changes.
pub(crate) struct Rows<R: Runtime> {
    pub(crate) dispatcher: MenuItem<R>,
    pub(crate) inbox: MenuItem<R>,
}

/// Polls the daemon and updates icon, tooltip and menu lines when something changed.
pub(crate) fn watch<R: Runtime>(app: &AppHandle<R>, rows: Rows<R>) {
    let app = app.clone();
    thread::spawn(move || {
        let mut shown: Option<Snapshot> = None;
        loop {
            let snapshot =
                Setup::from_env().map_or_else(|_| Snapshot::default(), |setup| fetch(&setup));
            if shown.as_ref() != Some(&snapshot) {
                if let Err(error) = show(&app, &rows, &snapshot) {
                    eprintln!("cannot update the tray: {error}");
                }
                shown = Some(snapshot);
            }
            thread::sleep(INTERVAL);
        }
    });
}

fn show<R: Runtime>(app: &AppHandle<R>, rows: &Rows<R>, snapshot: &Snapshot) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };
    let texts = texts(snapshot);
    tray.set_icon(Some(state(snapshot).icon()?))?;
    tray.set_tooltip(Some(&texts.tooltip))?;
    rows.dispatcher.set_text(&texts.dispatcher)?;
    rows.inbox.set_text(&texts.inbox)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::unnecessary_wraps)]
    fn dispatcher(enabled: bool, state: &str, message: &str) -> Option<DispatcherLine> {
        Some(DispatcherLine {
            enabled,
            state: state.to_owned(),
            message: message.to_owned(),
        })
    }

    #[allow(clippy::unnecessary_wraps)]
    fn inbox(questions: u32, others: u32) -> Option<InboxLine> {
        Some(InboxLine {
            questions,
            total: questions + others,
        })
    }

    #[test]
    fn a_question_comes_before_other_waiting_items() {
        let snapshot = Snapshot {
            dispatcher: dispatcher(true, "running", "running 1/2"),
            inbox: inbox(1, 3),
        };
        assert_eq!(state(&snapshot), TrayState::Question);
        let waiting = Snapshot {
            dispatcher: None,
            inbox: inbox(0, 2),
        };
        assert_eq!(state(&waiting), TrayState::Waiting);
    }

    #[test]
    fn nothing_waiting_or_no_daemon_is_ready() {
        let empty = Snapshot {
            dispatcher: None,
            inbox: inbox(0, 0),
        };
        assert_eq!(state(&empty), TrayState::Ready);
        assert_eq!(state(&Snapshot::default()), TrayState::Ready);
    }

    #[test]
    fn a_dispatcher_that_runs_does_not_change_the_icon() {
        let running = Snapshot {
            dispatcher: dispatcher(true, "running", "running 2/2"),
            inbox: inbox(0, 0),
        };
        assert_eq!(state(&running), TrayState::Ready);
    }

    #[test]
    fn the_lines_say_what_the_daemon_reported() {
        let full = texts(&Snapshot {
            dispatcher: dispatcher(true, "paused", "paused until 18:00"),
            inbox: inbox(1, 2),
        });
        assert_eq!(full.tooltip, "NULLRADIX - question open");
        assert_eq!(full.dispatcher, "Dispatcher: paused until 18:00");
        assert_eq!(full.inbox, "Open: 1 questions, 2 more");

        let off = texts(&Snapshot {
            dispatcher: dispatcher(false, "off", ""),
            inbox: inbox(0, 0),
        });
        assert_eq!(off.dispatcher, "Dispatcher: off");
        assert_eq!(off.tooltip, "NULLRADIX - ready");

        let bare = texts(&Snapshot {
            dispatcher: dispatcher(true, "idle", ""),
            inbox: None,
        });
        assert_eq!(bare.dispatcher, "Dispatcher: idle");
        assert_eq!(bare.inbox, "Open: unknown");
    }

    #[test]
    fn a_missing_daemon_and_a_missing_part_read_differently() {
        let gone = texts(&Snapshot::default());
        assert_eq!(gone.dispatcher, "Daemon not reachable");
        assert_eq!(gone.inbox, "Open: unknown");
        let partial = texts(&Snapshot {
            dispatcher: None,
            inbox: inbox(0, 1),
        });
        assert_eq!(partial.dispatcher, "Dispatcher: unknown");
    }

    #[test]
    fn a_stopped_daemon_gives_an_empty_snapshot() {
        let home = tempfile::tempdir().unwrap();
        let setup = Setup {
            profile: "dev",
            home: Some(home.path().to_path_buf()),
            daemon: home.path().join("nullradix"),
            web_dir: None,
        };
        assert_eq!(fetch(&setup), Snapshot::default());
    }

    #[test]
    fn every_icon_decodes() {
        for state in [TrayState::Ready, TrayState::Waiting, TrayState::Question] {
            assert!(state.icon().is_ok(), "{state:?}");
        }
    }
}
