//! Notices to the person: a desktop toast, later a web push.

use async_trait::async_trait;
use strum::IntoStaticStr;

use crate::AppError;

/// What a notice is about, as the legacy dispatcher named it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
pub enum NoticeKind {
    Merged,
    Blocked,
    Question,
    Paused,
    Review,
    /// An autopilot ticket is stuck in a dead end instead of blocked.
    Stuck,
    /// A merge waits for its folder without help from a person.
    MergeWaiting,
}

impl NoticeKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// One notice of the dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub kind: NoticeKind,
    /// Key of the ticket it is about.
    pub ticket: Option<String>,
    pub title: String,
    pub text: String,
}

impl Notice {
    /// A blocked ticket; an autopilot ticket is stuck instead.
    #[must_use]
    pub fn blocked(autopilot: bool, ticket: &str, title: &str, reason: &str) -> Self {
        let (kind, text) = if autopilot {
            (
                NoticeKind::Stuck,
                format!("Autopilot is stuck: {ticket} - {reason}"),
            )
        } else {
            (NoticeKind::Blocked, reason.to_owned())
        };
        Self {
            kind,
            ticket: Some(ticket.to_owned()),
            title: title.to_owned(),
            text,
        }
    }
}

#[async_trait]
pub trait Notifier: Send + Sync {
    /// Shows the notice; a failure never stops the work that caused it.
    async fn notify(&self, notice: &Notice) -> Result<(), AppError>;
}

/// Shows nothing, until a shell with a toast is attached.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoNotifier;

#[async_trait]
impl Notifier for NoNotifier {
    async fn notify(&self, _notice: &Notice) -> Result<(), AppError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_autopilot_ticket_is_stuck_not_blocked() {
        let stuck = Notice::blocked(true, "ARC-1", "Title", "no access");
        assert_eq!(stuck.kind, NoticeKind::Stuck);
        assert_eq!(stuck.text, "Autopilot is stuck: ARC-1 - no access");
        let blocked = Notice::blocked(false, "ARC-1", "Title", "no access");
        assert_eq!(blocked.kind, NoticeKind::Blocked);
        assert_eq!(blocked.text, "no access");
        assert_eq!(NoticeKind::MergeWaiting.as_str(), "merge-waiting");
    }
}
