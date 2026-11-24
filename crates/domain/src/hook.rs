use strum::{EnumIter, EnumString, IntoStaticStr};

use crate::{Id, Timestamp};

/// Prompts are kept this long; enough for titles and context, not a copy of the transcript.
const PROMPT_CHARS: usize = 10_000;
const DETAIL_CHARS: usize = 1_000;
const SESSION_CHARS: usize = 200;
const CWD_CHARS: usize = 4_096;

/// The Claude Code hooks NULLRADIX listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum HookKind {
    SessionStart,
    UserPrompt,
    Notification,
    Stop,
    SubagentStart,
    SubagentStop,
    SessionEnd,
}

impl HookKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// Whether Claude Code reads context from the answer to this hook.
    #[must_use]
    pub const fn answers(self) -> bool {
        matches!(self, Self::SessionStart | Self::UserPrompt)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HookError {
    #[error("a session id has 1 to 200 characters and no control characters")]
    InvalidSession,
    #[error("the working directory must be an absolute path of at most 4096 characters")]
    InvalidCwd,
}

/// Claude Code's id of one session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionRef(String);

impl SessionRef {
    pub fn parse(text: &str) -> Result<Self, HookError> {
        let trimmed = text.trim();
        let unusable = trimmed.chars().any(char::is_control);
        if trimmed.is_empty() || unusable || trimmed.chars().count() > SESSION_CHARS {
            return Err(HookError::InvalidSession);
        }
        Ok(Self(trimmed.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Optional facts a hook carries; which ones depends on its kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookDetails {
    pub transcript_path: Option<String>,
    pub prompt: Option<String>,
    pub source: Option<String>,
    pub model: Option<String>,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub notification_type: Option<String>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    /// Ticket key the session was started for, from `NULLRADIX_TICKET`.
    pub ticket: Option<String>,
    /// Run that started the session, from `NULLRADIX_RUN`.
    pub run: Option<String>,
    /// Which Claude client ran the hook, from `CLAUDE_CODE_ENTRYPOINT`.
    pub entrypoint: Option<String>,
}

impl HookDetails {
    /// Empty texts become absent and long ones are cut, since a hook must never be refused.
    #[must_use]
    pub fn clipped(self) -> Self {
        let detail = |text: Option<String>| clip(text, DETAIL_CHARS);
        Self {
            transcript_path: clip(self.transcript_path, CWD_CHARS),
            prompt: clip(self.prompt, PROMPT_CHARS),
            source: detail(self.source),
            model: detail(self.model),
            reason: detail(self.reason),
            message: detail(self.message),
            notification_type: detail(self.notification_type),
            agent_id: detail(self.agent_id),
            agent_type: detail(self.agent_type),
            ticket: detail(self.ticket),
            run: detail(self.run),
            entrypoint: detail(self.entrypoint),
        }
    }
}

fn clip(text: Option<String>, chars: usize) -> Option<String> {
    let text = text?;
    if text.trim().is_empty() {
        return None;
    }
    Some(match text.char_indices().nth(chars) {
        Some((end, _)) => text[..end].to_owned(),
        None => text,
    })
}

/// One report of a Claude Code hook; the id comes from the reporter, so a resend is no new event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEvent {
    id: Id<Self>,
    kind: HookKind,
    session: SessionRef,
    cwd: String,
    occurred_at: Timestamp,
    details: HookDetails,
}

impl HookEvent {
    pub fn new(
        id: Id<Self>,
        kind: HookKind,
        session: SessionRef,
        cwd: &str,
        occurred_at: Timestamp,
        details: HookDetails,
    ) -> Result<Self, HookError> {
        if cwd.chars().count() > CWD_CHARS || cwd.chars().any(char::is_control) || !is_absolute(cwd)
        {
            return Err(HookError::InvalidCwd);
        }
        Ok(Self {
            id,
            kind,
            session,
            cwd: cwd.to_owned(),
            occurred_at,
            details: details.clipped(),
        })
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.id
    }

    #[must_use]
    pub const fn kind(&self) -> HookKind {
        self.kind
    }

    #[must_use]
    pub const fn session(&self) -> &SessionRef {
        &self.session
    }

    #[must_use]
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    #[must_use]
    pub const fn occurred_at(&self) -> Timestamp {
        self.occurred_at
    }

    #[must_use]
    pub const fn details(&self) -> &HookDetails {
        &self.details
    }
}

/// Absolute on Windows or Unix, whichever machine the hook ran on.
fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    drive || path.starts_with("\\\\") || path.starts_with('/')
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::*;

    fn event(cwd: &str, details: HookDetails) -> Result<HookEvent, HookError> {
        HookEvent::new(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            HookKind::Stop,
            SessionRef::parse("abc").unwrap(),
            cwd,
            Timestamp::from_millis(5),
            details,
        )
    }

    #[test]
    fn kinds_have_wire_names_and_only_two_answer() {
        assert_eq!(HookKind::UserPrompt.as_str(), "user-prompt");
        assert_eq!("subagent-stop".parse(), Ok(HookKind::SubagentStop));
        let answering: Vec<HookKind> = HookKind::iter().filter(|kind| kind.answers()).collect();
        assert_eq!(
            answering,
            vec![HookKind::SessionStart, HookKind::UserPrompt]
        );
    }

    #[test]
    fn session_refs_are_trimmed_and_bounded() {
        assert_eq!(SessionRef::parse(" s1 ").unwrap().as_str(), "s1");
        assert_eq!(SessionRef::parse(" "), Err(HookError::InvalidSession));
        assert_eq!(
            SessionRef::parse(&"s".repeat(201)),
            Err(HookError::InvalidSession)
        );
        assert_eq!(SessionRef::parse("s\u{0}1"), Err(HookError::InvalidSession));
    }

    #[test]
    fn the_cwd_must_be_absolute_on_any_platform() {
        for cwd in ["C:\\repo", "d:/repo", "\\\\server\\share", "/home/me/repo"] {
            assert!(event(cwd, HookDetails::default()).is_ok(), "{cwd}");
        }
        for cwd in [
            "repo",
            "",
            "C:repo",
            "/re\u{0}po",
            &format!("/{}", "x".repeat(4096)),
        ] {
            assert_eq!(
                event(cwd, HookDetails::default()),
                Err(HookError::InvalidCwd),
                "{cwd}"
            );
        }
    }

    #[test]
    fn details_drop_blanks_and_cut_long_texts_at_characters() {
        let details = HookDetails {
            prompt: Some("\u{e9}".repeat(10_005)),
            model: Some(String::from("  ")),
            message: Some(String::from("done")),
            ..HookDetails::default()
        };
        let event = event("/repo", details).unwrap();
        let kept = event.details();
        assert_eq!(
            kept.prompt.as_deref().map(|p| p.chars().count()),
            Some(10_000)
        );
        assert_eq!(kept.model, None);
        assert_eq!(kept.message.as_deref(), Some("done"));
        assert_eq!(event.kind(), HookKind::Stop);
        assert_eq!(event.session().as_str(), "abc");
        assert_eq!(event.cwd(), "/repo");
        assert_eq!(event.occurred_at(), Timestamp::from_millis(5));
        assert_eq!(event.id(), Id::from_uuid(uuid::Uuid::from_u128(1)));
    }
}
