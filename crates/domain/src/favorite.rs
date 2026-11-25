//! Sessions kept to resume later, as a snapshot that outlives the pruned session.

use crate::{Id, Repository, Session, SessionRef, SessionState, TicketKey, Timestamp, Version};

/// Longer notes are cut, as legacy did.
pub const FAVORITE_NOTE_MAX: usize = 2000;

/// A later session that resumed the favorite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resumption {
    pub session: SessionRef,
    pub at: Timestamp,
}

/// Every field of a stored favorite, for persistence adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FavoriteRecord {
    pub session: SessionRef,
    pub repository: Option<Id<Repository>>,
    pub note: String,
    pub cwd: String,
    pub title: Option<String>,
    pub tickets: Vec<TicketKey>,
    pub last_state: SessionState,
    pub last_event_at: Timestamp,
    pub marked_at: Timestamp,
    /// Sessions that resumed this one, oldest first.
    pub resumed_as: Vec<Resumption>,
    pub version: Version,
}

/// A marked session with a note and what it looked like when last seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Favorite(FavoriteRecord);

/// The note as kept: cut to [`FAVORITE_NOTE_MAX`] characters.
#[must_use]
pub fn favorite_note(text: &str) -> String {
    text.chars().take(FAVORITE_NOTE_MAX).collect()
}

impl Favorite {
    /// Marks `session` with its `tickets` at `at`.
    #[must_use]
    pub fn mark(
        session: &Session,
        tickets: Vec<TicketKey>,
        note: Option<&str>,
        at: Timestamp,
    ) -> Self {
        let record = session.record();
        Self(FavoriteRecord {
            session: record.id.clone(),
            repository: record.repository,
            note: favorite_note(note.unwrap_or_default()),
            cwd: record.cwd.clone(),
            title: record.title.clone(),
            tickets,
            last_state: record.state,
            last_event_at: record.last_event_at,
            marked_at: at,
            resumed_as: Vec::new(),
            version: Version::INITIAL,
        })
    }

    /// The snapshot taken again; a given note replaces the old one, as marking twice did.
    #[must_use]
    pub fn remarked(&self, session: &Session, tickets: Vec<TicketKey>, note: Option<&str>) -> Self {
        let mut next = self.refreshed(session, tickets);
        if let Some(note) = note {
            next.0.note = favorite_note(note);
        }
        next
    }

    /// The snapshot after `session` changed; a title once known stays.
    #[must_use]
    pub fn refreshed(&self, session: &Session, tickets: Vec<TicketKey>) -> Self {
        let record = session.record();
        let mut next = self.0.clone();
        next.repository = record.repository.or(next.repository);
        next.tickets = tickets;
        record.cwd.clone_into(&mut next.cwd);
        if record.title.is_some() {
            next.title.clone_from(&record.title);
        }
        next.last_state = record.state;
        next.last_event_at = record.last_event_at;
        next.version = next.version.next();
        Self(next)
    }

    /// Only the note changes, for a favorite whose session is gone.
    #[must_use]
    pub fn with_note(&self, note: &str) -> Self {
        let mut next = self.0.clone();
        next.note = favorite_note(note);
        next.version = next.version.next();
        Self(next)
    }

    /// The favorite resumed as `session` at `at`; `None` when it is itself or already known.
    #[must_use]
    pub fn resumed(&self, session: &SessionRef, at: Timestamp) -> Option<Self> {
        let known = self
            .0
            .resumed_as
            .iter()
            .any(|resumption| &resumption.session == session);
        if &self.0.session == session || known {
            return None;
        }
        let mut next = self.0.clone();
        next.resumed_as.push(Resumption {
            session: session.clone(),
            at,
        });
        next.version = next.version.next();
        Some(Self(next))
    }

    /// The command to resume it, as legacy offered for copying.
    #[must_use]
    pub fn command(&self) -> String {
        resume_command(&self.0.cwd, self.0.session.as_str())
    }

    #[must_use]
    pub const fn record(&self) -> &FavoriteRecord {
        &self.0
    }

    /// For stores only: trusts the record as stored.
    #[must_use]
    pub const fn restore(record: FavoriteRecord) -> Self {
        Self(record)
    }
}

/// `cd "<folder>" && claude --resume <id>`; quotes in the folder are escaped, odd ids quoted.
#[must_use]
pub fn resume_command(cwd: &str, session: &str) -> String {
    let folder = format!("\"{}\"", cwd.replace('"', "\\\""));
    let plain = !session.is_empty()
        && session
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || matches!(letter, '.' | '_' | '-'));
    let id = if plain {
        session.to_owned()
    } else {
        format!("'{}'", session.replace('\'', "'\\''"))
    };
    format!("cd {folder} && claude --resume {id}")
}

/// Folder spelling for comparing: slashes, no trailing one, case folded where paths ignore case.
fn folder_key(path: &str, fold: bool) -> String {
    let cleaned = path.replace('\\', "/");
    let cleaned = cleaned.trim_end_matches('/');
    if fold {
        cleaned.to_lowercase()
    } else {
        cleaned.to_owned()
    }
}

/// The only favorite in `cwd` a resumed session belongs to; none when several or none match.
#[must_use]
pub fn resumed_favorite<'a>(
    folders: &'a [(SessionRef, String)],
    cwd: &str,
    fold: bool,
) -> Option<&'a SessionRef> {
    let key = folder_key(cwd, fold);
    let mut matching = folders
        .iter()
        .filter(|(_, folder)| folder_key(folder, fold) == key);
    match (matching.next(), matching.next()) {
        (Some((session, _)), None) => Some(session),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HookDetails, HookEvent, HookKind, ProjectKey};

    fn session(id: &str, title: Option<&str>, state: HookKind) -> Session {
        let event = |kind, at, prompt: Option<&str>| {
            HookEvent::new(
                Id::from_uuid(uuid::Uuid::from_u128(at)),
                kind,
                SessionRef::parse(id).unwrap(),
                "D:/My Project/wt",
                Timestamp::from_millis(i64::try_from(at).unwrap()),
                HookDetails {
                    prompt: prompt.map(str::to_owned),
                    ..HookDetails::default()
                },
            )
            .unwrap()
        };
        let start = event(HookKind::SessionStart, 1, None);
        let begun = Session::begin(&start, None).apply(&start).unwrap();
        let titled = match title {
            Some(title) => begun
                .apply(&event(HookKind::UserPrompt, 2, Some(title)))
                .unwrap(),
            None => begun,
        };
        titled.apply(&event(state, 3, None)).unwrap_or(titled)
    }

    fn key(number: u32) -> TicketKey {
        TicketKey::new(ProjectKey::parse("ARC").unwrap(), number)
    }

    #[test]
    fn marking_keeps_a_snapshot_and_a_cut_note() {
        let long = "x".repeat(FAVORITE_NOTE_MAX + 5);
        let favorite = Favorite::mark(
            &session("s1", Some("Login"), HookKind::Stop),
            vec![key(1)],
            Some(&long),
            Timestamp::from_millis(9),
        );
        let record = favorite.record();
        assert_eq!(record.note.chars().count(), FAVORITE_NOTE_MAX);
        assert_eq!(record.title.as_deref(), Some("Login"));
        assert_eq!(record.cwd, "D:/My Project/wt");
        assert_eq!(record.last_state, SessionState::Idle);
        assert_eq!(record.marked_at, Timestamp::from_millis(9));
        assert_eq!(record.version, Version::INITIAL);
    }

    #[test]
    fn marking_again_keeps_the_note_unless_given_and_the_title_once_known() {
        let first = Favorite::mark(
            &session("s1", Some("Login"), HookKind::Stop),
            vec![key(1)],
            Some("Note"),
            Timestamp::from_millis(9),
        );
        let ended = session("s1", None, HookKind::SessionEnd);
        let again = first.remarked(&ended, vec![key(1), key(2)], None);
        assert_eq!(again.record().note, "Note");
        assert_eq!(again.record().title.as_deref(), Some("Login"));
        assert_eq!(again.record().last_state, SessionState::Ended);
        assert_eq!(again.record().tickets, [key(1), key(2)]);
        assert_eq!(again.record().version, Version::INITIAL.next());
        assert_eq!(
            again
                .remarked(&ended, Vec::new(), Some("New"))
                .record()
                .note,
            "New"
        );
        assert_eq!(again.with_note("").record().note, "");
    }

    #[test]
    fn a_resumed_session_is_kept_once_and_never_itself() {
        let favorite = Favorite::mark(
            &session("s1", None, HookKind::Stop),
            Vec::new(),
            None,
            Timestamp::from_millis(1),
        );
        let new = SessionRef::parse("s2").unwrap();
        let at = Timestamp::from_millis(5);
        let resumed = favorite.resumed(&new, at).unwrap();
        assert_eq!(
            resumed.record().resumed_as,
            [Resumption {
                session: new.clone(),
                at
            }]
        );
        assert_eq!(resumed.resumed(&new, at), None);
        assert_eq!(
            favorite.resumed(&SessionRef::parse("s1").unwrap(), at),
            None
        );
    }

    #[test]
    fn the_resume_command_quotes_as_legacy() {
        assert_eq!(
            resume_command("D:/My Project/wt", "abc-123"),
            "cd \"D:/My Project/wt\" && claude --resume abc-123"
        );
        assert_eq!(
            resume_command("D:/a\"b", "x"),
            "cd \"D:/a\\\"b\" && claude --resume x"
        );
        assert_eq!(
            resume_command("/tmp", "a b;c"),
            "cd \"/tmp\" && claude --resume 'a b;c'"
        );
    }

    #[test]
    fn a_resumed_session_belongs_to_the_only_favorite_in_its_folder() {
        let id = |text: &str| SessionRef::parse(text).unwrap();
        let folders = vec![
            (id("a"), String::from("D:\\My Project\\wt\\")),
            (id("b"), String::from("D:/other")),
            (id("c"), String::from("D:/twice")),
            (id("d"), String::from("D:/twice")),
        ];
        assert_eq!(
            resumed_favorite(&folders, "D:/My Project/wt", false),
            Some(&id("a"))
        );
        assert_eq!(resumed_favorite(&folders, "d:/my project/WT", false), None);
        assert_eq!(
            resumed_favorite(&folders, "d:/my project/WT", true),
            Some(&id("a"))
        );
        assert_eq!(resumed_favorite(&folders, "D:/twice", false), None);
        assert_eq!(resumed_favorite(&folders, "D:/none", false), None);
    }
}
