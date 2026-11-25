//! Notes: findings about something other than the current ticket, kept for later tickets.

use std::collections::HashSet;

use strum::{EnumIter, EnumString, IntoStaticStr};

use crate::{Actor, Board, Id, Ticket, TicketKey, Timestamp, Version, utf16_len};

pub const NOTE_TITLE_MAX: usize = 200;
pub const NOTE_REASON_MAX: usize = 1000;
/// Length of the labels joined by commas, as the legacy column held them.
pub const NOTE_LABELS_MAX: usize = 1000;
/// Length of the other tickets a note was seen at, joined by commas.
pub const NOTE_SEEN_IN_MAX: usize = 1000;

/// From this share of shared terms, measured on the longer text, a note is the same finding.
const SIMILAR_SHARE: f64 = 0.25;
/// Fewer shared terms are chance, even for very short texts.
const SIMILAR_MIN_SHARED: usize = 3;

/// Filler words found in almost every note, as legacy listed them.
const STOP_WORDS: [&str; 58] = [
    "der",
    "die",
    "das",
    "und",
    "oder",
    "nicht",
    "von",
    "mit",
    "bei",
    "aus",
    "auf",
    "f\u{fc}r",
    "ist",
    "sind",
    "ein",
    "eine",
    "einer",
    "eines",
    "den",
    "dem",
    "des",
    "zu",
    "im",
    "in",
    "am",
    "an",
    "als",
    "auch",
    "wenn",
    "weil",
    "dass",
    "sich",
    "nur",
    "noch",
    "schon",
    "wird",
    "werden",
    "kein",
    "keine",
    "diese",
    "dieser",
    "dieses",
    "unabh\u{e4}ngig",
    "unabhaengig",
    "verursacht",
    "vermutlich",
    "durch",
    "nach",
    "vor",
    "\u{fc}ber",
    "unter",
    "sehr",
    "test",
    "tests",
    "npm",
    "run",
    "the",
    "and",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum NoteState {
    Open,
    /// Turned into a ticket; legacy called it taken.
    Promoted,
    Dropped,
}

impl NoteState {
    pub const ALL: [Self; 3] = [Self::Open, Self::Promoted, Self::Dropped];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    pub fn parse(text: &str) -> Result<Self, NoteError> {
        text.parse()
            .map_err(|_| NoteError::InvalidState(text.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NoteError {
    #[error("the note needs a title: one sentence on what stands out")]
    TitleEmpty,
    #[error("the note title is one line; details go into the body (--details)")]
    TitleMultiline,
    #[error(
        "the note title is too long ({0} characters, at most {NOTE_TITLE_MAX}); one short \
         sentence as the title, the rest into the body (--details)"
    )]
    TitleTooLong(usize),
    #[error("dropping a note needs a reason (--reason)")]
    ReasonMissing,
    #[error("the reason is too long ({0} characters, at most {NOTE_REASON_MAX})")]
    ReasonTooLong(usize),
    #[error("the labels of the note are too long ({0} characters, at most {NOTE_LABELS_MAX})")]
    LabelsTooLong(usize),
    #[error("state {0} is invalid; allowed: open, promoted, dropped, all")]
    InvalidState(String),
    #[error("{0} is no note number")]
    InvalidNumber(String),
}

/// Title to show; without one the first line of the body, else the number.
#[must_use]
pub fn shown_title(title: &str, body: &str, number: i64) -> String {
    let title = title.trim();
    if !title.is_empty() {
        return title.to_owned();
    }
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(|| format!("Note {number}"), str::to_owned)
}

/// The trimmed title of a new note: one line of at most [`NOTE_TITLE_MAX`] UTF-16 units.
pub fn note_title(text: &str) -> Result<String, NoteError> {
    let title = text.trim();
    if title.is_empty() {
        return Err(NoteError::TitleEmpty);
    }
    if title.contains('\n') {
        return Err(NoteError::TitleMultiline);
    }
    let length = utf16_len(title);
    if length > NOTE_TITLE_MAX {
        return Err(NoteError::TitleTooLong(length));
    }
    Ok(title.to_owned())
}

/// The trimmed reason a note is dropped for.
pub fn drop_reason(text: &str) -> Result<String, NoteError> {
    let reason = text.trim();
    if reason.is_empty() {
        return Err(NoteError::ReasonMissing);
    }
    let length = utf16_len(reason);
    if length > NOTE_REASON_MAX {
        return Err(NoteError::ReasonTooLong(length));
    }
    Ok(reason.to_owned())
}

/// Checks that normalized labels fit, joined by commas, into [`NOTE_LABELS_MAX`].
pub fn check_note_labels(labels: &[String]) -> Result<(), NoteError> {
    let length = utf16_len(&labels.join(","));
    if length > NOTE_LABELS_MAX {
        return Err(NoteError::LabelsTooLong(length));
    }
    Ok(())
}

/// A note number as typed: a whole number from 1.
pub fn note_number(text: &str) -> Result<i64, NoteError> {
    match text.parse::<i64>() {
        Ok(number) if number >= 1 => Ok(number),
        _ => Err(NoteError::InvalidNumber(text.to_owned())),
    }
}

/// A ticket a note points to; the key stays when the ticket is deleted, the id does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRef {
    pub id: Option<Id<Ticket>>,
    pub key: TicketKey,
}

impl TicketRef {
    #[must_use]
    pub fn of(ticket: &Ticket) -> Self {
        Self {
            id: Some(ticket.id()),
            key: ticket.key().clone(),
        }
    }
}

/// The finding was seen again, at a ticket or without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    pub ticket: Option<TicketRef>,
    pub at: Timestamp,
}

/// What a new note is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteDraft {
    pub board: Id<Board>,
    pub author: Id<Actor>,
    pub title: String,
    pub body: String,
    pub source: Option<TicketRef>,
    pub labels: Vec<String>,
}

/// A finding on a board; never deleted, only promoted to a ticket or dropped with a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: Id<Self>,
    /// What people type, such as 12; 0 until stored.
    pub number: i64,
    pub board: Id<Board>,
    pub author: Id<Actor>,
    pub title: String,
    pub body: String,
    /// The ticket it was found at.
    pub source: Option<TicketRef>,
    pub labels: Vec<String>,
    pub sightings: Vec<Sighting>,
    pub state: NoteState,
    pub promoted_to: Option<TicketRef>,
    pub reason: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

impl Note {
    #[must_use]
    pub fn new(id: Id<Self>, draft: NoteDraft, now: Timestamp) -> Self {
        Self {
            id,
            number: 0,
            board: draft.board,
            author: draft.author,
            title: draft.title,
            body: draft.body,
            source: draft.source,
            labels: draft.labels,
            sightings: Vec::new(),
            state: NoteState::Open,
            promoted_to: None,
            reason: None,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        }
    }

    /// How often the finding stood out: when noted and at every sighting.
    #[must_use]
    pub fn seen_count(&self) -> usize {
        1 + self.sightings.len()
    }

    /// Other tickets it was seen at, first sighting first; what no longer fits is only counted.
    #[must_use]
    pub fn seen_in(&self) -> Vec<&TicketKey> {
        let mut keys: Vec<&TicketKey> = Vec::new();
        let mut length = 0;
        for ticket in self
            .sightings
            .iter()
            .filter_map(|seen| seen.ticket.as_ref())
        {
            let key = &ticket.key;
            if self
                .source
                .as_ref()
                .is_some_and(|source| &source.key == key)
                || keys.contains(&key)
            {
                continue;
            }
            let added = key.to_string().len() + usize::from(!keys.is_empty());
            if length + added > NOTE_SEEN_IN_MAX {
                continue;
            }
            length += added;
            keys.push(key);
        }
        keys
    }

    /// Title to show; notes without one show the first line of their body.
    #[must_use]
    pub fn display_title(&self) -> String {
        shown_title(&self.title, &self.body, self.number)
    }

    fn revised(&self, now: Timestamp) -> Self {
        Self {
            updated_at: now,
            version: self.version.next(),
            ..self.clone()
        }
    }

    #[must_use]
    pub fn seen(&self, ticket: Option<TicketRef>, now: Timestamp) -> Self {
        let mut next = self.revised(now);
        next.sightings.push(Sighting { ticket, at: now });
        next
    }

    #[must_use]
    pub fn promoted(&self, ticket: TicketRef, now: Timestamp) -> Self {
        Self {
            state: NoteState::Promoted,
            promoted_to: Some(ticket),
            ..self.revised(now)
        }
    }

    #[must_use]
    pub fn dropped(&self, reason: String, now: Timestamp) -> Self {
        Self {
            state: NoteState::Dropped,
            reason: Some(reason),
            promoted_to: None,
            ..self.revised(now)
        }
    }

    /// Description of the ticket made from the note: its body and where it came from.
    #[must_use]
    pub fn ticket_description(&self) -> String {
        let origin = self
            .source
            .as_ref()
            .map_or_else(String::new, |source| format!(", seen at {}", source.key));
        let again = if self.seen_count() > 1 {
            let others = self.seen_in();
            let also = if others.is_empty() {
                String::new()
            } else {
                let keys: Vec<String> = others.iter().map(ToString::to_string).collect();
                format!(", also at {}", keys.join(", "))
            };
            format!("; seen {} times in total{also}", self.seen_count())
        } else {
            String::new()
        };
        let footer = format!("(note of {}{origin}{again})", self.created_at);
        if self.body.is_empty() {
            footer
        } else {
            format!("{}\n\n{footer}", self.body)
        }
    }

    /// Text the similarity check compares.
    fn text(&self) -> String {
        format!("{}\n{}", self.title, self.body)
    }
}

/// Notes that describe the same finding as `text`, most similar first; ties keep their order.
#[must_use]
pub fn similar_notes<'a>(text: &str, notes: impl IntoIterator<Item = &'a Note>) -> Vec<&'a Note> {
    let words = terms(text);
    let mut scored: Vec<(f64, &Note)> = notes
        .into_iter()
        .filter_map(|note| score(&words, &terms(&note.text())).map(|score| (score, note)))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().map(|(_, note)| note).collect()
}

/// Share of shared terms in the larger set; `None` below the thresholds.
fn score(a: &HashSet<String>, b: &HashSet<String>) -> Option<f64> {
    let shared = a.iter().filter(|word| b.contains(*word)).count();
    if shared < SIMILAR_MIN_SHARED {
        return None;
    }
    let larger = a.len().max(b.len());
    #[expect(
        clippy::cast_precision_loss,
        reason = "term counts stay far below 2^52"
    )]
    let share = shared as f64 / larger as f64;
    (share >= SIMILAR_SHARE).then_some(share)
}

/// Terms of a text: lower case, at least three letters, without ticket keys and filler words.
fn terms(text: &str) -> HashSet<String> {
    let lower = text.to_lowercase();
    let words = runs(&lower);
    let mut terms = HashSet::new();
    let mut index = 0;
    while index < words.len() {
        let (word, dash_after) = words[index];
        if dash_after
            && words
                .get(index + 1)
                .is_some_and(|(next, _)| key_number(next))
            && key_prefix(word)
        {
            index += 2;
            continue;
        }
        if word.chars().count() >= 3 && !STOP_WORDS.contains(&word) {
            terms.insert(word.to_owned());
        }
        index += 1;
    }
    terms
}

/// Runs of letters and digits, each with whether exactly one dash follows it before the next run.
fn runs(text: &str) -> Vec<(&str, bool)> {
    let mut found: Vec<(&str, bool)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut gap = String::new();
    for (at, letter) in text.char_indices() {
        if letter.is_alphanumeric() {
            if start.is_none() {
                if let Some(last) = found.last_mut() {
                    last.1 = gap == "-";
                }
                gap.clear();
                start = Some(at);
            }
        } else {
            if let Some(begin) = start.take() {
                found.push((&text[begin..at], false));
            }
            gap.push(letter);
        }
    }
    if let Some(begin) = start {
        found.push((&text[begin..], false));
    }
    found
}

/// The project part of a ticket or question key, such as `arc`.
fn key_prefix(word: &str) -> bool {
    let bytes = word.as_bytes();
    (2..=10).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// The number part of a key, such as `0007`, `q0007` or the legacy `f0007`.
fn key_number(word: &str) -> bool {
    let digits = word.strip_prefix(['q', 'f']).unwrap_or(word);
    digits.len() >= 3 && digits.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::ProjectKey;

    fn key(number: u32) -> TicketKey {
        TicketKey::new(ProjectKey::parse("ARC").unwrap(), number)
    }

    fn reference(number: u32) -> TicketRef {
        TicketRef {
            id: None,
            key: key(number),
        }
    }

    fn note(number: i64, title: &str, body: &str) -> Note {
        let draft = NoteDraft {
            board: Id::from_uuid(Uuid::from_u128(1)),
            author: Id::from_uuid(Uuid::from_u128(2)),
            title: title.to_owned(),
            body: body.to_owned(),
            source: None,
            labels: Vec::new(),
        };
        let mut note = Note::new(
            Id::from_uuid(Uuid::from_u128(10)),
            draft,
            Timestamp::from_millis(0),
        );
        note.number = number;
        note
    }

    #[test]
    fn titles_are_one_trimmed_line_of_limited_length() {
        assert_eq!(
            note_title("  Cache never expires ").unwrap(),
            "Cache never expires"
        );
        assert_eq!(note_title(" \n "), Err(NoteError::TitleEmpty));
        assert_eq!(note_title("one\ntwo"), Err(NoteError::TitleMultiline));
        assert_eq!(
            note_title(&"\u{1f600}".repeat(101)),
            Err(NoteError::TitleTooLong(202))
        );
        assert_eq!(note_title(&"a".repeat(200)).unwrap().len(), 200);
    }

    #[test]
    fn reasons_labels_and_numbers_have_limits() {
        assert_eq!(drop_reason(" done "), Ok(String::from("done")));
        assert_eq!(drop_reason("  "), Err(NoteError::ReasonMissing));
        assert_eq!(
            drop_reason(&"a".repeat(1001)),
            Err(NoteError::ReasonTooLong(1001))
        );
        let labels = vec!["a".repeat(500), "b".repeat(499)];
        assert_eq!(check_note_labels(&labels), Ok(()));
        let labels = vec!["a".repeat(500), "b".repeat(500)];
        assert_eq!(
            check_note_labels(&labels),
            Err(NoteError::LabelsTooLong(1001))
        );
        assert_eq!(note_number("12"), Ok(12));
        for text in ["0", "-1", "twelve", "", " 3"] {
            assert_eq!(
                note_number(text),
                Err(NoteError::InvalidNumber(text.to_owned()))
            );
        }
    }

    #[test]
    fn states_have_kebab_case_names() {
        let names: Vec<&str> = NoteState::ALL.iter().map(|state| state.as_str()).collect();
        assert_eq!(names, ["open", "promoted", "dropped"]);
        assert_eq!(NoteState::parse("promoted"), Ok(NoteState::Promoted));
        assert_eq!(
            NoteState::parse("taken").unwrap_err().to_string(),
            "state taken is invalid; allowed: open, promoted, dropped, all"
        );
    }

    #[test]
    fn display_titles_fall_back_to_the_first_body_line() {
        assert_eq!(
            note(1, " Cache broken ", "x").display_title(),
            "Cache broken"
        );
        assert_eq!(
            note(2, "", "\n  \nFirst filled line\nanother").display_title(),
            "First filled line"
        );
        assert_eq!(note(7, "", "   ").display_title(), "Note 7");
    }

    #[test]
    fn sightings_count_and_list_other_tickets_once() {
        let mut found = note(1, "Cache", "");
        found.source = Some(reference(1));
        let at = Timestamp::from_millis(5);
        let seen = found
            .seen(Some(reference(1)), at)
            .seen(Some(reference(2)), at)
            .seen(None, at)
            .seen(Some(reference(2)), at)
            .seen(Some(reference(3)), at);
        assert_eq!(seen.seen_count(), 6);
        assert_eq!(seen.seen_in(), [&key(2), &key(3)]);
        assert_eq!(seen.version, Version::from_stored(6));
        assert_eq!(seen.updated_at, at);
    }

    #[test]
    fn a_full_seen_list_only_counts() {
        let mut found = note(1, "Cache", "");
        let at = Timestamp::from_millis(5);
        // a key takes 8 units and each further one a comma: 111 keys fill 998 units
        for number in 1..=120 {
            found = found.seen(Some(reference(number)), at);
        }
        let seen = found.seen_in();
        assert_eq!(seen.len(), 111);
        assert_eq!(seen.last(), Some(&&key(111)));
        assert_eq!(found.seen_count(), 121);
    }

    #[test]
    fn promoting_and_dropping_change_state_and_version() {
        let open = note(3, "Cache", "");
        let at = Timestamp::from_millis(9);
        let promoted = open.promoted(reference(4), at);
        assert_eq!(promoted.state, NoteState::Promoted);
        assert_eq!(promoted.promoted_to, Some(reference(4)));
        assert_eq!(promoted.version, Version::from_stored(2));
        let dropped = open.dropped(String::from("gone"), at);
        assert_eq!(dropped.state, NoteState::Dropped);
        assert_eq!(dropped.reason.as_deref(), Some("gone"));
        assert_eq!(dropped.promoted_to, None);
    }

    #[test]
    fn ticket_descriptions_keep_the_body_and_the_origin() {
        let bare = note(1, "Cache", "");
        assert_eq!(
            bare.ticket_description(),
            "(note of 1970-01-01T00:00:00.000Z)"
        );
        let mut found = note(1, "Cache", "Expires never.");
        found.source = Some(reference(1));
        let found = found
            .seen(Some(reference(2)), Timestamp::from_millis(1))
            .seen(None, Timestamp::from_millis(2));
        assert_eq!(
            found.ticket_description(),
            "Expires never.\n\n(note of 1970-01-01T00:00:00.000Z, seen at ARC-0001; seen 3 times \
             in total, also at ARC-0002)"
        );
        let mut again = note(1, "Cache", "");
        again = again.seen(None, Timestamp::from_millis(1));
        assert_eq!(
            again.ticket_description(),
            "(note of 1970-01-01T00:00:00.000Z; seen 2 times in total)"
        );
    }

    #[test]
    fn similar_notes_share_terms_without_keys_and_filler_words() {
        let known = [
            note(
                1,
                "Dispatcher startet Worktree doppelt",
                "WorktreeManager legt Branch zweimal an",
            ),
            note(2, "Cache der Board-Ansicht l\u{e4}uft nie ab", ""),
        ];
        let found: Vec<i64> = similar_notes("WorktreeManager: Branch doppelt, Dispatcher", &known)
            .into_iter()
            .map(|note| note.number)
            .collect();
        assert_eq!(found, [1]);
        // two shared terms are chance
        assert_eq!(
            similar_notes("Dispatcher und Branch", &known),
            Vec::<&Note>::new()
        );
        // keys say nothing about the finding
        assert_eq!(
            similar_notes("ARC-0001 AIH-0022 ARC-F0001 ARC-Q0001", &known),
            Vec::<&Note>::new()
        );
    }

    #[test]
    fn the_larger_text_sets_the_share_and_better_matches_come_first() {
        let known = [
            note(
                1,
                "alpha beta gamma delta epsilon zeta eta theta",
                "iota kappa lambda omicron sigma",
            ),
            note(2, "alpha beta gamma", ""),
        ];
        let found: Vec<i64> = similar_notes("alpha beta gamma", &known)
            .into_iter()
            .map(|note| note.number)
            .collect();
        assert_eq!(found, [2]);
        let found: Vec<i64> = similar_notes("alpha beta gamma delta", &known)
            .into_iter()
            .map(|note| note.number)
            .collect();
        assert_eq!(found, [2, 1]);
    }

    #[test]
    fn key_parts_need_both_halves() {
        assert_eq!(terms("arc-0001"), HashSet::new());
        assert_eq!(
            terms("arc - 0001"),
            HashSet::from([String::from("arc"), String::from("0001")])
        );
        assert_eq!(terms("arc-12 x"), HashSet::from([String::from("arc")]));
        assert_eq!(
            terms("caf\u{e9}-0001 longword"),
            HashSet::from([
                String::from("caf\u{e9}"),
                String::from("0001"),
                String::from("longword")
            ])
        );
    }
}
