use std::time::Duration;

use strum::{EnumIter, EnumString, IntoStaticStr};

use crate::{
    HookEvent, HookKind, Id, Repository, Runner, SessionRef, Timestamp, Version, utf16_len,
    utf16_prefix,
};

/// Without a hook for this long a live session shows as stale.
pub const SESSION_STALE: Duration = Duration::from_mins(30);
/// Sessions whose last hook or end lies this far back are forgotten.
pub const SESSION_RETENTION: Duration = Duration::from_hours(24 * 7);

/// Notifications that mean Claude waits for the person.
const WAITING_NOTIFICATIONS: [&str; 2] = ["permission_prompt", "idle_prompt"];
/// Blocks Claude Code or the IDE put into a prompt, not typed by the person.
const INJECTED_NAMES: [&str; 2] = ["system-reminder", "browser_instruction"];
const INJECTED_PREFIXES: [&str; 3] = ["ide_", "command-", "local-command-"];
const TITLE_WORDS: usize = 6;
const TITLE_UNITS: usize = 50;
const UNKNOWN_AGENT: &str = "unknown";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum SessionState {
    Idle,
    Running,
    Waiting,
    Ended,
}

impl SessionState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// What a list shows: the state, or stale when no hook came for a while.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum DisplayState {
    Idle,
    Running,
    Waiting,
    Ended,
    Stale,
}

impl DisplayState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

impl From<SessionState> for DisplayState {
    fn from(state: SessionState) -> Self {
        match state {
            SessionState::Idle => Self::Idle,
            SessionState::Running => Self::Running,
            SessionState::Waiting => Self::Waiting,
            SessionState::Ended => Self::Ended,
        }
    }
}

/// Where a session comes from: a NULLRADIX run, Claude Code or the Claude desktop app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum SessionClient {
    Nullradix,
    ClaudeCode,
    ClaudeDesktop,
    Unknown,
}

impl SessionClient {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// The client behind `CLAUDE_CODE_ENTRYPOINT`: the desktop app, else Claude Code.
    #[must_use]
    pub fn from_entrypoint(entrypoint: &str) -> Self {
        if entrypoint == "claude-desktop" {
            Self::ClaudeDesktop
        } else {
            Self::ClaudeCode
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum SubagentState {
    Running,
    Ended,
}

impl SubagentState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// A helper agent a session started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subagent {
    pub id: String,
    pub kind: String,
    pub state: SubagentState,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
}

/// Every field of a stored session, for persistence adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: SessionRef,
    pub repository: Option<Id<Repository>>,
    pub runner: Option<Id<Runner>>,
    /// Per repository, given once the repository is known.
    pub number: Option<u32>,
    pub cwd: String,
    pub state: SessionState,
    pub client: SessionClient,
    pub title: Option<String>,
    pub model: Option<String>,
    pub transcript_path: Option<String>,
    pub run_name: Option<String>,
    pub started_at: Timestamp,
    pub last_event_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub subagents: Vec<Subagent>,
    pub version: Version,
}

/// Figures a list computes at read time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionView {
    pub display_state: DisplayState,
    /// Whether the session still answers: idle, running or waiting and not stale.
    pub listening: bool,
    pub runtime: Duration,
}

/// One Claude Code session, built from its hook events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session(SessionRecord);

impl Session {
    /// An idle session first seen with `event`, before the event is applied.
    #[must_use]
    pub fn begin(event: &HookEvent, runner: Option<Id<Runner>>) -> Self {
        let at = event.occurred_at();
        Self(SessionRecord {
            id: event.session().clone(),
            repository: None,
            runner,
            number: None,
            cwd: event.cwd().to_owned(),
            state: SessionState::Idle,
            client: SessionClient::Unknown,
            title: None,
            model: None,
            transcript_path: None,
            run_name: None,
            started_at: at,
            last_event_at: at,
            ended_at: None,
            subagents: Vec::new(),
            version: Version::INITIAL,
        })
    }

    /// The session after `event`; `None` when the event is older than the last one seen.
    #[must_use]
    pub fn apply(&self, event: &HookEvent) -> Option<Self> {
        let at = event.occurred_at();
        if at < self.0.last_event_at {
            return None;
        }
        let mut next = self.0.clone();
        next.last_event_at = at;
        event.cwd().clone_into(&mut next.cwd);
        let details = event.details();
        if let Some(model) = &details.model {
            next.model = Some(model.clone());
        }
        if event.kind() == HookKind::UserPrompt && next.title.is_none() {
            let title = title_from_prompt(details.prompt.as_deref());
            next.title = (!title.is_empty()).then_some(title);
        }
        take_origin(&mut next, event);
        transition(&mut next, event);
        next.version = next.version.next();
        Some(Self(next))
    }

    /// The session placed in `repository` with its `number`.
    #[must_use]
    pub fn located(&self, repository: Id<Repository>, number: u32) -> Self {
        let mut next = self.0.clone();
        next.repository = Some(repository);
        next.number = Some(number);
        next.version = next.version.next();
        Self(next)
    }

    /// State, listening and runtime as of `now`.
    #[must_use]
    pub fn view(&self, now: Timestamp) -> SessionView {
        let record = &self.0;
        let stale = record.state != SessionState::Ended
            && now.saturating_since(record.last_event_at) > SESSION_STALE;
        let display_state = if stale {
            DisplayState::Stale
        } else {
            record.state.into()
        };
        SessionView {
            display_state,
            listening: matches!(
                display_state,
                DisplayState::Idle | DisplayState::Running | DisplayState::Waiting
            ),
            runtime: record
                .ended_at
                .unwrap_or(now)
                .saturating_since(record.started_at),
        }
    }

    /// Whether the session is old enough to forget as of `now`.
    #[must_use]
    pub fn expired(&self, now: Timestamp) -> bool {
        let last = self.0.ended_at.unwrap_or(self.0.last_event_at);
        now.saturating_since(last) > SESSION_RETENTION
    }

    /// Name shown in lists, such as `SE-001: Fix the login`.
    #[must_use]
    pub fn label(&self) -> String {
        session_label(self.0.number, self.0.title.as_deref())
    }

    #[must_use]
    pub const fn id(&self) -> &SessionRef {
        &self.0.id
    }

    #[must_use]
    pub const fn record(&self) -> &SessionRecord {
        &self.0
    }

    /// For stores only: trusts the record as stored.
    #[must_use]
    pub const fn restore(record: SessionRecord) -> Self {
        Self(record)
    }
}

/// Moment before which ended or silent sessions are forgotten.
#[must_use]
pub fn session_prune_before(now: Timestamp) -> Timestamp {
    let keep = i64::try_from(SESSION_RETENTION.as_millis()).unwrap_or(i64::MAX);
    Timestamp::from_millis(now.as_millis().saturating_sub(keep))
}

/// Transcript, run and client; blanks change nothing and a NULLRADIX run stays one.
fn take_origin(session: &mut SessionRecord, event: &HookEvent) {
    let given = |value: &Option<String>| value.clone().filter(|text| !text.trim().is_empty());
    let details = event.details();
    if let Some(path) = given(&details.transcript_path) {
        session.transcript_path = Some(path);
    }
    if let Some(run) = given(&details.run) {
        session.run_name = Some(run);
        session.client = SessionClient::Nullradix;
    }
    if let Some(entrypoint) = given(&details.entrypoint)
        && session.client != SessionClient::Nullradix
    {
        session.client = SessionClient::from_entrypoint(&entrypoint);
    }
}

fn transition(session: &mut SessionRecord, event: &HookEvent) {
    let at = event.occurred_at();
    let details = event.details();
    match event.kind() {
        HookKind::SessionStart => {
            session.state = SessionState::Idle;
            session.ended_at = None;
        }
        HookKind::UserPrompt => session.state = SessionState::Running,
        HookKind::Notification => {
            let kind = details.notification_type.as_deref().unwrap_or_default();
            let idle_while_agents_run = kind == "idle_prompt"
                && session
                    .subagents
                    .iter()
                    .any(|agent| agent.state == SubagentState::Running);
            if WAITING_NOTIFICATIONS.contains(&kind) && !idle_while_agents_run {
                session.state = SessionState::Waiting;
            }
        }
        HookKind::Stop => {
            session.state = SessionState::Idle;
            end_subagents(session, at);
        }
        HookKind::SubagentStart => {
            session.state = SessionState::Running;
            upsert_subagent(session, event, SubagentState::Running);
        }
        HookKind::SubagentStop => upsert_subagent(session, event, SubagentState::Ended),
        HookKind::SessionEnd => {
            session.state = SessionState::Ended;
            session.ended_at = Some(at);
            end_subagents(session, at);
        }
    }
}

fn upsert_subagent(session: &mut SessionRecord, event: &HookEvent, state: SubagentState) {
    let at = event.occurred_at();
    let details = event.details();
    let id = details.agent_id.clone().unwrap_or_else(|| {
        format!(
            "{}-{}",
            details.agent_type.as_deref().unwrap_or("agent"),
            session.subagents.len() + 1
        )
    });
    let index = if let Some(index) = session.subagents.iter().position(|agent| agent.id == id) {
        index
    } else {
        session.subagents.push(Subagent {
            id,
            kind: details
                .agent_type
                .clone()
                .unwrap_or_else(|| UNKNOWN_AGENT.to_owned()),
            state,
            started_at: at,
            ended_at: None,
        });
        session.subagents.len() - 1
    };
    let agent = &mut session.subagents[index];
    agent.state = state;
    if state == SubagentState::Ended {
        agent.ended_at = Some(at);
    }
}

fn end_subagents(session: &mut SessionRecord, at: Timestamp) {
    for agent in &mut session.subagents {
        if agent.state == SubagentState::Running {
            agent.state = SubagentState::Ended;
            agent.ended_at = Some(at);
        }
    }
}

/// `SE-001: title`, only `SE-001` without a title; a numbered title stays as it is.
#[must_use]
pub fn session_label(number: Option<u32>, title: Option<&str>) -> String {
    let text = title.unwrap_or_default().trim();
    if numbered(text) {
        return text.to_owned();
    }
    let Some(number) = number else {
        return text.to_owned();
    };
    let label = format!("SE-{number:03}");
    if text.is_empty() {
        label
    } else {
        format!("{label}: {text}")
    }
}

fn numbered(text: &str) -> bool {
    text.strip_prefix("SE-")
        .is_some_and(|rest| rest.starts_with(|letter: char| letter.is_ascii_digit()))
}

/// A short title from the first words of a prompt; empty when only injected blocks remain.
#[must_use]
pub fn title_from_prompt(prompt: Option<&str>) -> String {
    let stripped = strip_injected(prompt.unwrap_or_default());
    let first_line = stripped
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    let words: Vec<&str> = first_line.split_whitespace().collect();
    let text = words
        .iter()
        .take(TITLE_WORDS)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let cut = if utf16_len(&text) > TITLE_UNITS {
        utf16_prefix(&text, TITLE_UNITS).trim_end().to_owned()
    } else {
        text.clone()
    };
    let shortened = words.len() > TITLE_WORDS || utf16_len(&cut) < utf16_len(&text);
    if !cut.is_empty() && shortened {
        format!("{cut} ...")
    } else {
        cut
    }
}

/// Drops injected blocks with their content; a tag without its end tag stays.
fn strip_injected(prompt: &str) -> String {
    let mut rest = prompt;
    let mut kept = String::with_capacity(prompt.len());
    while let Some(start) = rest.find('<') {
        kept.push_str(&rest[..start]);
        let block = &rest[start..];
        if let Some(length) = block_len(block) {
            rest = &block[length..];
        } else {
            kept.push('<');
            rest = &block[1..];
        }
    }
    kept.push_str(rest);
    kept
}

fn block_len(block: &str) -> Option<usize> {
    let name_length = block[1..]
        .find(|letter: char| !is_name_char(letter))
        .unwrap_or(block.len() - 1);
    let name = &block[1..=name_length];
    if !is_injected(name) {
        return None;
    }
    let after_name = &block[1 + name_length..];
    let open_end = match after_name.chars().next()? {
        '>' => name_length + 2,
        letter if letter.is_whitespace() => 1 + name_length + after_name.find('>')? + 1,
        _ => return None,
    };
    let end_tag = format!("</{name}>");
    let end = block[open_end..].find(&end_tag)?;
    Some(open_end + end + end_tag.len())
}

fn is_name_char(letter: char) -> bool {
    letter.is_ascii_alphanumeric() || letter == '_' || letter == '-'
}

fn is_injected(name: &str) -> bool {
    INJECTED_NAMES.contains(&name)
        || INJECTED_PREFIXES.iter().any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|rest| !rest.is_empty())
        })
}

/// Whether `path` is `folder` or lies inside it, compared as the runner's system does.
#[must_use]
pub fn path_within(folder: &str, path: &str, case_insensitive: bool) -> bool {
    let normalize = |text: &str| {
        let slashed = text.replace('\\', "/");
        let trimmed = slashed.trim_end_matches('/').to_owned();
        if case_insensitive {
            trimmed.to_lowercase()
        } else {
            trimmed
        }
    };
    let (folder, path) = (normalize(folder), normalize(path));
    !folder.is_empty() && (path == folder || path.starts_with(&format!("{folder}/")))
}

/// Ticket keys a hook names, as `NULLRADIX_TICKET` lists them comma separated.
#[must_use]
pub fn ticket_names(text: Option<&str>) -> Vec<String> {
    text.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::HookDetails;

    use super::*;

    const START: i64 = 1_791_626_400_000;

    struct Clocked {
        session: Option<Session>,
        now: i64,
        serial: u128,
    }

    impl Clocked {
        fn new() -> Self {
            Self {
                session: None,
                now: START,
                serial: 0,
            }
        }

        fn advance(&mut self, millis: i64) {
            self.now += millis;
        }

        fn event(&mut self, kind: HookKind, details: HookDetails) -> HookEvent {
            self.serial += 1;
            HookEvent::new(
                Id::from_uuid(uuid::Uuid::from_u128(self.serial)),
                kind,
                SessionRef::parse("s1").unwrap(),
                "D:/wt/T-0042",
                Timestamp::from_millis(self.now),
                details,
            )
            .unwrap()
        }

        fn apply_with(&mut self, kind: HookKind, details: HookDetails) -> Session {
            let event = self.event(kind, details);
            let current = self
                .session
                .take()
                .unwrap_or_else(|| Session::begin(&event, None));
            let next = current.apply(&event).unwrap();
            self.session = Some(next.clone());
            next
        }

        fn apply(&mut self, kind: HookKind) -> Session {
            self.apply_with(kind, HookDetails::default())
        }

        fn note(&mut self, kind: &str) -> Session {
            self.apply_with(
                HookKind::Notification,
                HookDetails {
                    notification_type: Some(kind.to_owned()),
                    ..HookDetails::default()
                },
            )
        }

        fn agent(&mut self, kind: HookKind, id: &str, agent_type: &str) -> Session {
            self.apply_with(
                kind,
                HookDetails {
                    agent_id: Some(id.to_owned()),
                    agent_type: Some(agent_type.to_owned()),
                    ..HookDetails::default()
                },
            )
        }
    }

    fn state(session: &Session) -> SessionState {
        session.record().state
    }

    #[test]
    fn a_session_start_creates_an_idle_session_with_model_and_cwd() {
        let mut clock = Clocked::new();
        let session = clock.apply_with(
            HookKind::SessionStart,
            HookDetails {
                model: Some(String::from("m")),
                ..HookDetails::default()
            },
        );
        let record = session.record();
        assert_eq!(record.id.as_str(), "s1");
        assert_eq!(record.cwd, "D:/wt/T-0042");
        assert_eq!(record.state, SessionState::Idle);
        assert_eq!(record.model.as_deref(), Some("m"));
        assert_eq!(record.started_at, Timestamp::from_millis(START));
        assert_eq!(record.version, Version::from_stored(2));
    }

    #[test]
    fn prompt_runs_notification_waits_stop_idles_and_end_ends() {
        let mut clock = Clocked::new();
        clock.apply(HookKind::SessionStart);
        clock.advance(1000);
        assert_eq!(
            state(&clock.apply(HookKind::UserPrompt)),
            SessionState::Running
        );
        assert_eq!(
            state(&clock.note("permission_prompt")),
            SessionState::Waiting
        );
        assert_eq!(
            state(&clock.apply(HookKind::UserPrompt)),
            SessionState::Running
        );
        assert_eq!(state(&clock.note("idle_prompt")), SessionState::Waiting);
        assert_eq!(state(&clock.apply(HookKind::Stop)), SessionState::Idle);
        clock.advance(5000);
        let ended = clock.apply(HookKind::SessionEnd);
        assert_eq!(state(&ended), SessionState::Ended);
        assert_eq!(
            ended.record().ended_at,
            Some(Timestamp::from_millis(START + 6000))
        );
    }

    #[test]
    fn other_notifications_keep_the_state() {
        let mut clock = Clocked::new();
        clock.apply(HookKind::UserPrompt);
        clock.advance(60_000);
        let session = clock.note("auth_success");
        assert_eq!(state(&session), SessionState::Running);
        assert_eq!(
            session.record().last_event_at,
            Timestamp::from_millis(START + 60_000)
        );
    }

    #[test]
    fn any_event_creates_an_unknown_session() {
        let mut clock = Clocked::new();
        assert_eq!(
            state(&clock.apply(HookKind::UserPrompt)),
            SessionState::Running
        );
        let mut other = Clocked::new();
        assert_eq!(state(&other.apply(HookKind::Stop)), SessionState::Idle);
    }

    #[test]
    fn a_new_session_start_resets_without_losing_the_start() {
        let mut clock = Clocked::new();
        clock.apply(HookKind::SessionStart);
        clock.apply(HookKind::SessionEnd);
        clock.advance(10_000);
        let session = clock.apply(HookKind::SessionStart);
        assert_eq!(state(&session), SessionState::Idle);
        assert_eq!(session.record().ended_at, None);
        assert_eq!(session.record().started_at, Timestamp::from_millis(START));
    }

    #[test]
    fn subagents_start_and_end_and_set_the_session_running() {
        let mut clock = Clocked::new();
        clock.apply(HookKind::SessionStart);
        clock.advance(1000);
        let session = clock.agent(HookKind::SubagentStart, "a1", "general-purpose");
        assert_eq!(state(&session), SessionState::Running);
        assert_eq!(
            session.record().subagents,
            [Subagent {
                id: String::from("a1"),
                kind: String::from("general-purpose"),
                state: SubagentState::Running,
                started_at: Timestamp::from_millis(START + 1000),
                ended_at: None,
            }]
        );
        clock.advance(4000);
        let session = clock.agent(HookKind::SubagentStop, "a1", "general-purpose");
        let agent = &session.record().subagents[0];
        assert_eq!(agent.state, SubagentState::Ended);
        assert_eq!(agent.ended_at, Some(Timestamp::from_millis(START + 5000)));
    }

    #[test]
    fn a_subagent_stop_without_start_records_an_ended_subagent() {
        let mut clock = Clocked::new();
        let session = clock.agent(HookKind::SubagentStop, "x", "Explore");
        assert_eq!(session.record().subagents.len(), 1);
        assert_eq!(session.record().subagents[0].state, SubagentState::Ended);
    }

    #[test]
    fn subagents_without_id_or_type_get_a_numbered_id_and_unknown_type() {
        let mut clock = Clocked::new();
        let session = clock.apply(HookKind::SubagentStart);
        let agent = &session.record().subagents[0];
        assert_eq!(agent.id, "agent-1");
        assert_eq!(agent.kind, "unknown");
    }

    #[test]
    fn idle_hints_while_subagents_work_keep_the_session_running() {
        let mut clock = Clocked::new();
        clock.agent(HookKind::SubagentStart, "a1", "t");
        assert_eq!(state(&clock.note("idle_prompt")), SessionState::Running);
        assert_eq!(
            state(&clock.note("permission_prompt")),
            SessionState::Waiting
        );
        clock.agent(HookKind::SubagentStop, "a1", "t");
        assert_eq!(state(&clock.note("idle_prompt")), SessionState::Waiting);
    }

    #[test]
    fn stop_and_session_end_end_running_subagents() {
        let mut clock = Clocked::new();
        clock.agent(HookKind::SubagentStart, "a1", "t");
        let stopped = clock.apply(HookKind::Stop);
        assert_eq!(stopped.record().subagents[0].state, SubagentState::Ended);
        clock.agent(HookKind::SubagentStart, "a2", "t");
        let ended = clock.apply(HookKind::SessionEnd);
        assert!(
            ended
                .record()
                .subagents
                .iter()
                .all(|agent| agent.state == SubagentState::Ended)
        );
    }

    #[test]
    fn older_events_change_nothing() {
        let mut clock = Clocked::new();
        clock.advance(1000);
        let session = clock.apply(HookKind::UserPrompt);
        clock.advance(-500);
        let late = clock.event(HookKind::SessionEnd, HookDetails::default());
        assert_eq!(session.apply(&late), None);
    }

    #[test]
    fn stale_after_thirty_minutes_without_hooks_unless_ended() {
        let mut clock = Clocked::new();
        let running = clock.apply(HookKind::UserPrompt);
        let ended = clock.apply(HookKind::SessionEnd);
        let later = Timestamp::from_millis(START + 31 * 60 * 1000);
        assert_eq!(running.view(later).display_state, DisplayState::Stale);
        assert_eq!(running.record().state, SessionState::Running);
        assert_eq!(ended.view(later).display_state, DisplayState::Ended);
    }

    #[test]
    fn idle_running_and_waiting_sessions_listen() {
        let now = Timestamp::from_millis(START);
        let listening = |kind: HookKind| {
            let mut clock = Clocked::new();
            clock
                .apply_with(
                    kind,
                    HookDetails {
                        notification_type: Some(String::from("permission_prompt")),
                        ..HookDetails::default()
                    },
                )
                .view(now)
                .listening
        };
        assert_eq!(
            [
                listening(HookKind::SessionStart),
                listening(HookKind::UserPrompt),
                listening(HookKind::Notification),
                listening(HookKind::SessionEnd),
            ],
            [true, true, true, false]
        );
        let mut clock = Clocked::new();
        let running = clock.apply(HookKind::UserPrompt);
        let later = Timestamp::from_millis(START + 31 * 60 * 1000);
        assert!(!running.view(later).listening);
    }

    #[test]
    fn runtime_runs_until_the_end_or_now() {
        let mut clock = Clocked::new();
        let started = clock.apply(HookKind::SessionStart);
        clock.advance(90_000);
        let now = Timestamp::from_millis(clock.now);
        assert_eq!(started.view(now).runtime, Duration::from_secs(90));
        let ended = clock.apply(HookKind::SessionEnd);
        let later = Timestamp::from_millis(clock.now + 60_000);
        assert_eq!(ended.view(later).runtime, Duration::from_secs(90));
    }

    #[test]
    fn sessions_expire_seven_days_after_their_end_or_last_hook() {
        let mut clock = Clocked::new();
        let session = clock.apply(HookKind::SessionStart);
        let week = 7 * 24 * 60 * 60 * 1000;
        assert!(!session.expired(Timestamp::from_millis(START + week - 1000)));
        assert!(session.expired(Timestamp::from_millis(START + week + 1000)));
        assert_eq!(
            session_prune_before(Timestamp::from_millis(START + week)),
            Timestamp::from_millis(START)
        );
    }

    #[test]
    fn takes_transcript_and_client_and_ignores_blanks() {
        let mut clock = Clocked::new();
        let session = clock.apply_with(
            HookKind::SessionStart,
            HookDetails {
                transcript_path: Some(String::from("C:/t/s1.jsonl")),
                entrypoint: Some(String::from("claude-desktop")),
                ..HookDetails::default()
            },
        );
        assert_eq!(
            session.record().transcript_path.as_deref(),
            Some("C:/t/s1.jsonl")
        );
        assert_eq!(session.record().client, SessionClient::ClaudeDesktop);
        assert_eq!(session.record().run_name, None);
        let session = clock.apply(HookKind::Stop);
        assert_eq!(
            session.record().transcript_path.as_deref(),
            Some("C:/t/s1.jsonl")
        );
        assert_eq!(session.record().client, SessionClient::ClaudeDesktop);
    }

    #[test]
    fn a_session_with_a_run_stays_a_nullradix_session() {
        let mut clock = Clocked::new();
        let session = clock.apply_with(
            HookKind::SessionStart,
            HookDetails {
                run: Some(String::from("ARC-0001-20261104-091500.log")),
                entrypoint: Some(String::from("cli")),
                ..HookDetails::default()
            },
        );
        assert_eq!(session.record().client, SessionClient::Nullradix);
        let session = clock.apply_with(
            HookKind::UserPrompt,
            HookDetails {
                entrypoint: Some(String::from("claude-desktop")),
                ..HookDetails::default()
            },
        );
        assert_eq!(session.record().client, SessionClient::Nullradix);
        assert_eq!(
            session.record().run_name.as_deref(),
            Some("ARC-0001-20261104-091500.log")
        );
    }

    #[test]
    fn the_first_titled_prompt_names_the_session() {
        let mut clock = Clocked::new();
        let prompt = |text: &str| HookDetails {
            prompt: Some(text.to_owned()),
            ..HookDetails::default()
        };
        let blank = clock.apply_with(
            HookKind::UserPrompt,
            prompt("<system-reminder>x</system-reminder>"),
        );
        assert_eq!(blank.record().title, None);
        clock.apply_with(HookKind::UserPrompt, prompt("Fix the login"));
        let kept = clock.apply_with(HookKind::UserPrompt, prompt("Now the logout"));
        assert_eq!(kept.record().title.as_deref(), Some("Fix the login"));
        assert_eq!(kept.label(), "Fix the login");
        let repository = Id::from_uuid(uuid::Uuid::from_u128(9));
        let located = kept.located(repository, 3);
        assert_eq!(located.label(), "SE-003: Fix the login");
        assert_eq!(located.record().repository, Some(repository));
    }

    #[test]
    fn clients_follow_the_entrypoint() {
        assert_eq!(
            SessionClient::from_entrypoint("claude-desktop"),
            SessionClient::ClaudeDesktop
        );
        assert_eq!(
            SessionClient::from_entrypoint("cli"),
            SessionClient::ClaudeCode
        );
        assert_eq!(
            SessionClient::from_entrypoint("sdk-cli"),
            SessionClient::ClaudeCode
        );
        assert_eq!(
            "claude-code".parse::<SessionClient>(),
            Ok(SessionClient::ClaudeCode)
        );
        assert!("nexus".parse::<SessionClient>().is_err());
    }

    #[test]
    fn labels_put_the_three_digit_number_before_the_title() {
        assert_eq!(
            session_label(Some(1), Some("Build login")),
            "SE-001: Build login"
        );
        assert_eq!(session_label(Some(12), None), "SE-012");
        assert_eq!(session_label(Some(12), Some("  ")), "SE-012");
        assert_eq!(
            session_label(Some(3), Some("SE-001: Build login")),
            "SE-001: Build login"
        );
        assert_eq!(session_label(None, None), "");
    }

    #[test]
    fn titles_take_the_first_words_of_the_prompt() {
        assert_eq!(
            title_from_prompt(Some("\n  Build   a login page\nwith a password")),
            "Build a login page"
        );
        assert_eq!(
            title_from_prompt(Some("one two three four five six seven")),
            "one two three four five six ..."
        );
        assert_eq!(title_from_prompt(Some("   ")), "");
        assert_eq!(title_from_prompt(None), "");
    }

    #[test]
    fn titles_skip_injected_blocks() {
        let prompt = [
            "<system-reminder>",
            "Codebase and rules",
            "</system-reminder>",
            "<browser_instruction># Claude in Chrome",
            "Use the tools.",
            "</browser_instruction>Setting the session names does not work",
        ]
        .join("\n");
        assert_eq!(
            title_from_prompt(Some(&prompt)),
            "Setting the session names does not ..."
        );
        assert_eq!(
            title_from_prompt(Some("<system-reminder>\nonly context\n</system-reminder>")),
            ""
        );
        let ide = concat!(
            "<ide_selection file=\"a.rs\">Line 1</ide_selection>",
            "<local-command-stdout>Output</local-command-stdout>",
            "<command-name>/foo</command-name>Title after"
        );
        assert_eq!(title_from_prompt(Some(ide)), "Title after");
    }

    #[test]
    fn a_tag_without_its_end_stays() {
        assert_eq!(
            title_from_prompt(Some("<system-reminder>without end")),
            "<system-reminder>without end"
        );
        assert_eq!(title_from_prompt(Some("<only a tag>")), "<only a tag>");
    }

    #[test]
    fn titles_are_cut_at_fifty_units() {
        assert_eq!(
            title_from_prompt(Some(
                "aaaaaaaaaa bbbbbbbbbb cccccccccc dddddddddd eeeeeeeeee"
            )),
            "aaaaaaaaaa bbbbbbbbbb cccccccccc dddddddddd eeeeee ..."
        );
        assert_eq!(
            title_from_prompt(Some(
                "aaaaaaaaaa bbbbbbbbbb cccccccccc dddddddddddddddd eeeee"
            )),
            "aaaaaaaaaa bbbbbbbbbb cccccccccc dddddddddddddddd ..."
        );
    }

    #[test]
    fn paths_lie_within_folders_by_whole_segments() {
        assert!(path_within("D:/wt/T-1", "D:\\wt\\T-1\\src", false));
        assert!(path_within("D:/wt/T-1/", "D:/wt/T-1", false));
        assert!(!path_within("D:/wt/T-1", "D:/wt/T-10", false));
        assert!(!path_within("D:/wt/T-1", "d:/wt/t-1", false));
        assert!(path_within("D:/wt/T-1", "d:/wt/t-1", true));
        assert!(!path_within("", "/x", false));
    }

    #[test]
    fn ticket_names_are_split_at_commas() {
        assert_eq!(ticket_names(Some(" ARC-1, ,ARC-2")), ["ARC-1", "ARC-2"]);
        assert_eq!(ticket_names(None), Vec::<String>::new());
    }
}
