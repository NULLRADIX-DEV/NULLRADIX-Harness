//! Notes: findings kept for later tickets, promoted to a ticket or dropped with a reason.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Actor, ActorKind, ActorName, Board, BoardError, Id, NOTE_TITLE_MAX, Note, NoteDraft, NoteError,
    NoteState, Project, Ticket, TicketKey, TicketRef, Version, check_note_labels, drop_reason,
    normalize_labels, note_number, note_title, similar_notes, utf16_prefix,
};

use crate::boards::{self, ActorRepository};
use crate::projects::ProjectRepository;
use crate::tickets::{
    self, Kind, NewTicket, Placement, TicketService, TicketView, TicketWrite, WriteOptions,
};
use crate::{AppError, ChangeSignal, Clock, IdGenerator, SimilarNote};

/// How many similar notes a refusal names, as in legacy.
const SIMILAR_SHOWN: usize = 3;

/// A note with the name of its author.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRow {
    pub note: Note,
    pub author_name: String,
}

/// Note changes inside a ticket write, so a promotion commits with its ticket.
#[async_trait]
pub trait NoteWrite: TicketWrite + Send {
    /// Notes of the board in one of `states`, by number.
    async fn board_notes(
        &mut self,
        board: Id<Board>,
        states: &[NoteState],
    ) -> Result<Vec<Note>, AppError>;
    async fn note(&mut self, number: i64) -> Result<Option<NoteRow>, AppError>;
    /// Stores a new note; the result carries the number it got.
    async fn insert_note(&mut self, note: &Note) -> Result<(i64, crate::Change), AppError>;
    /// Stores the note with its labels and sightings if it still has `expected`.
    async fn update_note(
        &mut self,
        note: &Note,
        expected: Version,
    ) -> Result<crate::Change, AppError>;
}

#[async_trait]
pub trait NoteRepository: Send + Sync {
    async fn begin(&self) -> Result<Box<dyn NoteWrite>, AppError>;
    /// Notes of the board by number; `None` when the project has no such board.
    async fn list(
        &self,
        project: Id<Project>,
        board: Id<Board>,
        states: &[NoteState],
        source: Option<&TicketKey>,
    ) -> Result<Option<Vec<NoteRow>>, AppError>;
    async fn get(&self, number: i64) -> Result<Option<NoteRow>, AppError>;
}

/// A new note; the body holds the details, `source` the ticket it was found at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewNote {
    pub title: String,
    pub body: Option<String>,
    pub source: Option<String>,
    /// Entries may hold several labels split by commas.
    pub labels: Vec<String>,
    /// Adds it even when a similar note exists.
    pub force: bool,
}

/// What the ticket made from a note gets instead of the note's values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Promotion {
    pub title: Option<String>,
    /// Backlog when left out.
    pub status: Option<String>,
    /// Normal when left out.
    pub priority: Option<String>,
    /// Replaces the note's labels.
    pub labels: Option<Vec<String>>,
}

/// Who writes: a person in the browser or an agent with a token, under an optional name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Writer<'a> {
    pub by_person: bool,
    /// Name to write under, such as `claude`; the acting person when left out or blank.
    pub name: Option<&'a str>,
}

/// A promoted note and the ticket made from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotedNote {
    pub note: NoteRow,
    pub ticket: TicketView,
}

#[must_use]
pub fn invalid(error: &NoteError) -> AppError {
    let code = match error {
        NoteError::TitleEmpty => "note-empty",
        NoteError::TitleMultiline => "note-title-multiline",
        NoteError::TitleTooLong(_) | NoteError::ReasonTooLong(_) | NoteError::LabelsTooLong(_) => {
            "value-too-long"
        }
        NoteError::ReasonMissing => "reason-missing",
        NoteError::InvalidState(_) => "state-invalid",
        NoteError::InvalidNumber(_) => "note-id-invalid",
    };
    AppError::invalid(code, error.to_string())
}

#[must_use]
pub fn not_found(number: i64) -> AppError {
    AppError::not_found("note-not-found", format!("note {number} does not exist"))
}

/// States a list shows: one state, `all`, or only open ones when left out.
pub fn states(text: Option<&str>) -> Result<Vec<NoteState>, AppError> {
    match text {
        None => Ok(vec![NoteState::Open]),
        Some("all") => Ok(NoteState::ALL.to_vec()),
        Some(text) => NoteState::parse(text)
            .map(|state| vec![state])
            .map_err(|error| invalid(&error)),
    }
}

fn number(text: &str) -> Result<i64, AppError> {
    note_number(text).map_err(|error| invalid(&error))
}

fn author_invalid(error: &BoardError) -> AppError {
    let text = match error {
        BoardError::ActorName(text) | BoardError::Name(text) => *text,
    };
    AppError::text(text, "author-empty", error.to_string())
}

fn promoted_key(note: &Note) -> String {
    note.promoted_to
        .as_ref()
        .map_or_else(String::new, |ticket| ticket.key.to_string())
}

fn already_promoted(note: &Note, hint: &str) -> AppError {
    AppError::conflict(
        "note-promoted",
        format!(
            "note {} is already promoted to {}{hint}",
            note.number,
            promoted_key(note)
        ),
    )
}

fn other_project(ticket: &Ticket, what: &str) -> AppError {
    AppError::invalid(
        "note-source-other-project",
        format!("{} belongs to another project than {what}", ticket.key()),
    )
}

/// The refusal for a finding that is probably noted already, naming the candidates.
fn similar(notes: &[&Note], source: Option<&TicketKey>) -> AppError {
    let mut lines = vec![String::from("this finding probably exists already:")];
    for note in notes {
        let state = if note.state == NoteState::Promoted {
            format!("[promoted to {}]", promoted_key(note))
        } else {
            String::from("[open]")
        };
        lines.push(format!(
            "  {}  {state} {}",
            note.number,
            note.display_title()
        ));
    }
    let open = notes.iter().find(|note| note.state == NoteState::Open);
    let promoted = notes.iter().find(|note| note.state == NoteState::Promoted);
    if let Some(open) = open {
        let from = source.map_or_else(String::new, |key| format!(" --from {key}"));
        lines.push(format!(
            "the same finding: nullradix note seen {}{from}",
            open.number
        ));
    } else if let Some(promoted) = promoted {
        lines.push(format!(
            "already a ticket: comment on {} instead of noting it again",
            promoted_key(promoted)
        ));
    }
    lines.push(String::from(
        "something else after all: add it with --force",
    ));
    let candidates = notes
        .iter()
        .map(|note| SimilarNote {
            number: note.number,
            title: note.display_title(),
            state: note.state,
            ticket: note
                .promoted_to
                .as_ref()
                .map(|ticket| ticket.key.to_string()),
        })
        .collect();
    AppError::conflict("note-similar", lines.join("\n")).with_similar(candidates)
}

/// The ticket a note names as its origin.
async fn origin(write: &mut dyn TicketWrite, text: &str) -> Result<Ticket, AppError> {
    let key = TicketKey::parse(text).map_err(|error| tickets::invalid(&error))?;
    write
        .ticket(&key)
        .await?
        .ok_or_else(|| tickets::not_found(&key))
}

async fn load(write: &mut dyn NoteWrite, number: i64) -> Result<NoteRow, AppError> {
    write.note(number).await?.ok_or_else(|| not_found(number))
}

/// The board of a stored note, which the board's deletion would have taken along.
async fn board_of(
    write: &mut dyn NoteWrite,
    note: &Note,
) -> Result<(Board, nullradix_domain::ProjectKey), AppError> {
    write
        .board(note.board)
        .await?
        .ok_or_else(|| boards::not_found(note.board))
}

pub struct NoteService {
    notes: Arc<dyn NoteRepository>,
    tickets: Arc<TicketService>,
    projects: Arc<dyn ProjectRepository>,
    actors: Arc<dyn ActorRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for NoteService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoteService").finish_non_exhaustive()
    }
}

impl NoteService {
    #[must_use]
    pub fn new(
        notes: Arc<dyn NoteRepository>,
        tickets: Arc<TicketService>,
        projects: Arc<dyn ProjectRepository>,
        actors: Arc<dyn ActorRepository>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            notes,
            tickets,
            projects,
            actors,
            clock,
            ids,
            signal,
        }
    }

    /// Notes an open finding; a similar open or promoted note refuses it unless forced.
    pub async fn add(
        &self,
        project: Id<Project>,
        board: Id<Board>,
        input: &NewNote,
        writer: Writer<'_>,
        person: &Actor,
    ) -> Result<NoteRow, AppError> {
        let title = note_title(&input.title).map_err(|error| invalid(&error))?;
        let body = input.body.as_deref().unwrap_or_default().trim().to_owned();
        // the actor commits on its own, before the note write takes the connection
        let author = self.writer(writer, person).await?;
        let labels =
            normalize_labels(&input.labels).map_err(|error| tickets::broken_rule(&error))?;
        check_note_labels(&labels).map_err(|error| invalid(&error))?;
        let mut write = self.notes.begin().await?;
        let found = write
            .board(board)
            .await?
            .filter(|(found, _)| found.project() == project);
        let Some((found, _)) = found else {
            drop(write);
            return Err(boards::missing(self.projects.as_ref(), project, board).await);
        };
        let source = match input.source.as_deref() {
            Some(text) => Some(origin(write.as_mut(), text).await?),
            None => None,
        };
        if let Some(ticket) = &source
            && ticket.record().project != found.project()
        {
            let what = format!(
                "board {board}; the note belongs on the ticket's board (--board {})",
                ticket.record().board
            );
            return Err(other_project(ticket, &what));
        }
        if !input.force {
            let known = write
                .board_notes(board, &[NoteState::Open, NoteState::Promoted])
                .await?;
            let found = similar_notes(&format!("{title}\n{body}"), &known);
            if !found.is_empty() {
                let shown = &found[..found.len().min(SIMILAR_SHOWN)];
                return Err(similar(shown, source.as_ref().map(Ticket::key)));
            }
        }
        let draft = NoteDraft {
            board,
            author: author.id(),
            title,
            body,
            source: source.as_ref().map(TicketRef::of),
            labels,
        };
        let mut note = Note::new(self.ids.next(), draft, self.clock.now());
        let (number, change) = write.insert_note(&note).await?;
        write.commit().await?;
        self.signal.notify(&change);
        note.number = number;
        Ok(NoteRow {
            note,
            author_name: author.name().as_str().to_owned(),
        })
    }

    /// The finding of an open note stood out again: counts it and remembers the ticket.
    pub async fn seen(&self, number: &str, source: Option<&str>) -> Result<NoteRow, AppError> {
        let number = self::number(number)?;
        let mut write = self.notes.begin().await?;
        let row = load(write.as_mut(), number).await?;
        let note = &row.note;
        match note.state {
            NoteState::Promoted => return Err(already_promoted(note, "; comment there")),
            NoteState::Dropped => {
                return Err(AppError::conflict(
                    "note-not-open",
                    format!(
                        "note {number} is dropped ({})",
                        note.reason.as_deref().unwrap_or_default()
                    ),
                ));
            }
            NoteState::Open => {}
        }
        let (board, _) = board_of(write.as_mut(), note).await?;
        let ticket = match source {
            Some(text) => Some(origin(write.as_mut(), text).await?),
            None => None,
        };
        if let Some(ticket) = &ticket
            && ticket.record().project != board.project()
        {
            return Err(other_project(ticket, &format!("note {number}")));
        }
        let seen = note.seen(ticket.as_ref().map(TicketRef::of), self.clock.now());
        let change = write.update_note(&seen, note.version).await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(NoteRow {
            note: seen,
            author_name: row.author_name,
        })
    }

    /// Notes of the board by number; only open ones unless a state or `all` is given.
    pub async fn list(
        &self,
        project: Id<Project>,
        board: Id<Board>,
        state: Option<&str>,
        source: Option<&str>,
    ) -> Result<Vec<NoteRow>, AppError> {
        let states = states(state)?;
        let source = source
            .map(TicketKey::parse)
            .transpose()
            .map_err(|error| tickets::invalid(&error))?;
        match self
            .notes
            .list(project, board, &states, source.as_ref())
            .await?
        {
            Some(rows) => Ok(rows),
            None => Err(boards::missing(self.projects.as_ref(), project, board).await),
        }
    }

    pub async fn get(&self, number: &str) -> Result<NoteRow, AppError> {
        let number = self::number(number)?;
        self.notes
            .get(number)
            .await?
            .ok_or_else(|| not_found(number))
    }

    /// Makes a ticket on the note's board from an open note, in one transaction.
    pub async fn promote(
        &self,
        number: &str,
        promotion: &Promotion,
        writer: Writer<'_>,
        person: &Actor,
    ) -> Result<PromotedNote, AppError> {
        let number = self::number(number)?;
        let kind = Kind::of(&NewTicket {
            status: promotion.status.clone(),
            priority: promotion.priority.clone(),
            ..NewTicket::default()
        })?;
        let actor = self.writer(writer, person).await?;
        let mut write = self.notes.begin().await?;
        let row = load(write.as_mut(), number).await?;
        let note = &row.note;
        let refused = match note.state {
            NoteState::Open => None,
            NoteState::Promoted => Some(format!("already promoted to {}", promoted_key(note))),
            NoteState::Dropped => Some(String::from("dropped")),
        };
        if let Some(why) = refused {
            return Err(AppError::conflict(
                "note-not-open",
                format!("note {number} is {why}"),
            ));
        }
        let input = NewTicket {
            title: promotion
                .title
                .clone()
                .unwrap_or_else(|| utf16_prefix(&note.display_title(), NOTE_TITLE_MAX)),
            description: Some(note.ticket_description()),
            labels: promotion
                .labels
                .clone()
                .unwrap_or_else(|| note.labels.clone()),
            ..NewTicket::default()
        };
        let (board, project_key) = board_of(write.as_mut(), note).await?;
        let placement = Placement {
            board: &board,
            project_key,
            kind,
        };
        let (ticket, _) = self
            .tickets
            .insert_new(
                write.as_mut(),
                placement,
                &input,
                &actor,
                WriteOptions::default(),
            )
            .await?;
        let promoted = note.promoted(TicketRef::of(&ticket.ticket), self.clock.now());
        let change = write.update_note(&promoted, note.version).await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(PromotedNote {
            note: NoteRow {
                note: promoted,
                author_name: row.author_name,
            },
            ticket,
        })
    }

    /// Drops a note with a reason; promoted notes stay promoted, dropped ones take the new reason.
    pub async fn drop_note(&self, number: &str, reason: &str) -> Result<NoteRow, AppError> {
        let number = self::number(number)?;
        let reason = drop_reason(reason).map_err(|error| invalid(&error))?;
        let mut write = self.notes.begin().await?;
        let row = load(write.as_mut(), number).await?;
        let note = &row.note;
        if note.state == NoteState::Promoted {
            return Err(already_promoted(note, ""));
        }
        let dropped = note.dropped(reason, self.clock.now());
        let change = write.update_note(&dropped, note.version).await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(NoteRow {
            note: dropped,
            author_name: row.author_name,
        })
    }

    /// The named writer, as a person or a session by who writes; else the acting person.
    async fn writer(&self, writer: Writer<'_>, person: &Actor) -> Result<Actor, AppError> {
        let Some(name) = writer.name.filter(|name| !name.trim().is_empty()) else {
            return Ok(person.clone());
        };
        let name = ActorName::parse(name).map_err(|error| author_invalid(&error))?;
        let kind = if writer.by_person {
            ActorKind::Human
        } else {
            ActorKind::Session
        };
        let candidate = Actor::new(self.ids.next(), kind, name, self.clock.now());
        self.actors.ensure(&candidate).await
    }
}

#[cfg(test)]
mod tests {
    use nullradix_domain::{NoteDraft, ProjectKey, TextError, Timestamp};
    use uuid::Uuid;

    use super::*;

    fn note(number: i64, state: NoteState) -> Note {
        let draft = NoteDraft {
            board: Id::from_uuid(Uuid::from_u128(1)),
            author: Id::from_uuid(Uuid::from_u128(2)),
            title: format!("Finding {number}"),
            body: String::new(),
            source: None,
            labels: Vec::new(),
        };
        let mut note = Note::new(
            Id::from_uuid(Uuid::from_u128(3)),
            draft,
            Timestamp::from_millis(0),
        );
        note.number = number;
        note.state = state;
        if state == NoteState::Promoted {
            note.promoted_to = Some(TicketRef {
                id: None,
                key: TicketKey::new(ProjectKey::parse("ARC").unwrap(), 9),
            });
        }
        note
    }

    #[test]
    fn maps_note_errors_to_codes() {
        let cases = [
            (NoteError::TitleEmpty, "note-empty"),
            (NoteError::TitleMultiline, "note-title-multiline"),
            (NoteError::TitleTooLong(201), "value-too-long"),
            (NoteError::ReasonMissing, "reason-missing"),
            (NoteError::ReasonTooLong(1001), "value-too-long"),
            (NoteError::LabelsTooLong(1001), "value-too-long"),
            (NoteError::InvalidState(String::from("x")), "state-invalid"),
            (
                NoteError::InvalidNumber(String::from("x")),
                "note-id-invalid",
            ),
        ];
        for (error, code) in cases {
            assert_eq!(invalid(&error).code(), code);
        }
        assert_eq!(
            author_invalid(&BoardError::ActorName(TextError::TooLong(100))).code(),
            "value-too-long"
        );
        assert_eq!(
            not_found(4).to_string(),
            "note-not-found: note 4 does not exist"
        );
    }

    #[test]
    fn list_states_default_to_open() {
        assert_eq!(states(None).unwrap(), [NoteState::Open]);
        assert_eq!(states(Some("all")).unwrap(), NoteState::ALL);
        assert_eq!(states(Some("dropped")).unwrap(), [NoteState::Dropped]);
        assert_eq!(states(Some("offen")).unwrap_err().code(), "state-invalid");
    }

    #[test]
    fn similar_refusals_point_to_seen_or_to_the_ticket() {
        let open = note(1, NoteState::Open);
        let promoted = note(2, NoteState::Promoted);
        let key = TicketKey::new(ProjectKey::parse("ARC").unwrap(), 3);
        let error = similar(&[&promoted, &open], Some(&key));
        assert_eq!(error.code(), "note-similar");
        assert_eq!(
            error.message(),
            "this finding probably exists already:\n  2  [promoted to ARC-0009] Finding 2\n  1  \
             [open] Finding 1\nthe same finding: nullradix note seen 1 --from ARC-0003\n\
             something else after all: add it with --force"
        );
        assert_eq!(error.similar().len(), 2);
        assert_eq!(error.similar()[0].ticket.as_deref(), Some("ARC-0009"));
        let error = similar(&[&promoted], None);
        assert!(
            error
                .message()
                .contains("already a ticket: comment on ARC-0009 instead of noting it again"),
            "{}",
            error.message()
        );
    }
}
