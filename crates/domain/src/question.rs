//! Questions to a person: asked on a project, answered or discarded by a person only.

use std::fmt;

use strum::{EnumIter, EnumString, IntoStaticStr};

use crate::{
    Actor, ActorKind, Id, Project, ProjectKey, TextError, TicketKey, Timestamp, Version,
    optional_text, required_text, utf16_len, utf16_prefix,
};

pub const QUESTION_TITLE_MAX: usize = 400;
pub const OCCASION_MAX: usize = 400;
pub const OPTION_TITLE_MAX: usize = 400;
/// Options one round may propose, as legacy `MAX_OPTIONS`.
pub const OPTIONS_MAX: usize = 5;
pub const SESSION_MAX: usize = 100;
/// The only answer no person gives: the dispatcher's autopilot takes a recommended option.
pub const AUTOPILOT_PREFIX: &str = "Autopilot:";
/// Room of a note in a ticket's history, as legacy `History.Note`.
const TICKET_NOTE_MAX: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuestionError {
    #[error("{0} is no question key, expected something like ARC-Q0007")]
    InvalidKey(String),
    #[error("{0} is neither a question such as ARC-Q0001 nor a ticket such as ARC-0001")]
    InvalidReference(String),
    #[error("state {0} is unknown, allowed are open, answered and discarded")]
    InvalidState(String),
    #[error("the title {0}")]
    Title(TextError),
    #[error("the occasion {0}")]
    Occasion(TextError),
    #[error("the session id {0}")]
    Session(TextError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ResearchError {
    #[error("a round proposes at most {OPTIONS_MAX} options")]
    TooManyOptions,
    #[error("at most one option of a round may be recommended")]
    RecommendedMultiple,
    #[error("every option needs a title and a text")]
    OptionIncomplete,
    #[error("the title of an option has more than {OPTION_TITLE_MAX} characters")]
    OptionTitleTooLong,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecisionError {
    #[error("a person makes this decision; run it in the web UI or in an own terminal")]
    HumanOnly,
    #[error("question {0} is not open; reopen it first")]
    NotOpen(QuestionKey),
    #[error("question {0} is open already")]
    AlreadyOpen(QuestionKey),
    #[error("choose either an option or an own answer, not both and not none")]
    Ambiguous,
    #[error("option {0} is not one of the newest round")]
    UnknownOption(String),
    #[error("give a reason")]
    ReasonMissing,
    #[error("a reason starting with {AUTOPILOT_PREFIX} is kept for the dispatcher's autopilot")]
    AutopilotReasonReserved,
}

/// The human key of a question, such as `ARC-Q0007`; numbers grow past four digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QuestionKey {
    project: ProjectKey,
    number: u32,
}

impl QuestionKey {
    #[must_use]
    pub const fn new(project: ProjectKey, number: u32) -> Self {
        Self { project, number }
    }

    /// Accepts any case and unpadded numbers, so `arc-q7` is `ARC-Q0007`.
    pub fn parse(text: &str) -> Result<Self, QuestionError> {
        let invalid = || QuestionError::InvalidKey(text.to_owned());
        let (key, rest) = text.trim().rsplit_once('-').ok_or_else(invalid)?;
        let digits = rest.strip_prefix(['Q', 'q']).ok_or_else(invalid)?;
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        let number = digits.parse::<u32>().map_err(|_| invalid())?;
        let project = ProjectKey::parse(&key.to_ascii_uppercase()).map_err(|_| invalid())?;
        Ok(Self { project, number })
    }

    #[must_use]
    pub const fn project(&self) -> &ProjectKey {
        &self.project
    }

    #[must_use]
    pub const fn number(&self) -> u32 {
        self.number
    }
}

impl fmt::Display for QuestionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-Q{:04}", self.project, self.number)
    }
}

/// What the answer to a question waits for: another question or a ticket.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum QuestionRef {
    Question(QuestionKey),
    Ticket(TicketKey),
}

impl QuestionRef {
    pub fn parse(text: &str) -> Result<Self, QuestionError> {
        if let Ok(key) = QuestionKey::parse(text) {
            return Ok(Self::Question(key));
        }
        TicketKey::parse(text)
            .map(Self::Ticket)
            .map_err(|_| QuestionError::InvalidReference(text.to_owned()))
    }
}

impl fmt::Display for QuestionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Question(key) => key.fmt(f),
            Self::Ticket(key) => key.fmt(f),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "lowercase")]
pub enum QuestionState {
    Open,
    Answered,
    Discarded,
}

impl QuestionState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    pub fn parse(text: &str) -> Result<Self, QuestionError> {
        text.parse()
            .map_err(|_| QuestionError::InvalidState(text.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionTitle(String);

impl QuestionTitle {
    pub fn parse(text: &str) -> Result<Self, QuestionError> {
        required_text(text, QUESTION_TITLE_MAX)
            .map(|title| Self(title.to_owned()))
            .map_err(QuestionError::Title)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why the question came up, such as `planning ARC-0007`; blank is none.
pub fn occasion(text: &str) -> Result<Option<String>, QuestionError> {
    optional_text(text, OCCASION_MAX)
        .map(|text| text.map(str::to_owned))
        .map_err(QuestionError::Occasion)
}

/// The agent session that asks; blank is none.
pub fn session(text: &str) -> Result<Option<String>, QuestionError> {
    optional_text(text, SESSION_MAX)
        .map(|text| text.map(str::to_owned))
        .map_err(QuestionError::Session)
}

/// An option as it comes in; the number is given by the question.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptionDraft {
    pub title: String,
    pub text: String,
    pub pros: Vec<String>,
    pub cons: Vec<String>,
    pub recommended: bool,
}

/// A proposed answer, numbered O1, O2 ... across all rounds of one question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOption {
    pub number: u32,
    pub title: String,
    pub text: String,
    pub pros: Vec<String>,
    pub cons: Vec<String>,
    pub recommended: bool,
}

impl QuestionOption {
    /// The key people choose by, such as `O2`.
    #[must_use]
    pub fn key(&self) -> String {
        format!("O{}", self.number)
    }
}

/// One round of options with its context; asking with options writes the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Research {
    pub id: Id<Self>,
    pub question: Id<Question>,
    pub actor: Id<Actor>,
    pub context: String,
    pub options: Vec<QuestionOption>,
    pub created_at: Timestamp,
}

fn texts(items: &[String]) -> Vec<String> {
    items
        .iter()
        .map(|item| item.trim().to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Checks the options of a new round and numbers them after those of `earlier` rounds.
pub fn number_options(
    drafts: &[OptionDraft],
    earlier: &[Research],
) -> Result<Vec<QuestionOption>, ResearchError> {
    if drafts.len() > OPTIONS_MAX {
        return Err(ResearchError::TooManyOptions);
    }
    if drafts.iter().filter(|draft| draft.recommended).count() > 1 {
        return Err(ResearchError::RecommendedMultiple);
    }
    let highest = earlier
        .iter()
        .flat_map(|round| round.options.iter().map(|option| option.number))
        .max()
        .unwrap_or(0);
    let mut options = Vec::with_capacity(drafts.len());
    for (number, draft) in (highest + 1..).zip(drafts) {
        let title = draft.title.trim();
        let text = draft.text.trim();
        if title.is_empty() || text.is_empty() {
            return Err(ResearchError::OptionIncomplete);
        }
        if utf16_len(title) > OPTION_TITLE_MAX {
            return Err(ResearchError::OptionTitleTooLong);
        }
        options.push(QuestionOption {
            number,
            title: title.to_owned(),
            text: text.to_owned(),
            pros: texts(&draft.pros),
            cons: texts(&draft.cons),
            recommended: draft.recommended,
        });
    }
    Ok(options)
}

/// What an answer chose: an option of the newest round or an own text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Option(u32),
    Text(String),
}

/// A person's decision; a discarded question has no choice but a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub choice: Option<Choice>,
    pub reason: Option<String>,
    pub by: Id<Actor>,
    pub at: Timestamp,
}

impl Decision {
    /// Taken over by the autopilot rather than decided by a person.
    #[must_use]
    pub fn by_autopilot(&self) -> bool {
        matches!(self.choice, Some(Choice::Option(_)))
            && self
                .reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with(AUTOPILOT_PREFIX))
    }
}

/// An answer as typed: exactly one of option and text, the reason is free.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnswerInput<'a> {
    pub option: Option<&'a str>,
    pub text: Option<&'a str>,
    pub reason: Option<&'a str>,
}

fn trimmed(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|text| !text.is_empty())
}

impl AnswerInput<'_> {
    fn by_autopilot(&self) -> bool {
        trimmed(self.option).is_some()
            && trimmed(self.text).is_none()
            && trimmed(self.reason).is_some_and(|reason| reason.starts_with(AUTOPILOT_PREFIX))
    }
}

fn option_number(text: &str) -> Option<u32> {
    let digits = text.strip_prefix(['O', 'o'])?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Every stored field of a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionRecord {
    pub id: Id<Question>,
    pub key: QuestionKey,
    pub project: Id<Project>,
    pub title: QuestionTitle,
    pub description: String,
    pub state: QuestionState,
    pub occasion: Option<String>,
    pub tags: Vec<String>,
    /// Tickets the question is about, sorted; it holds them back while open.
    pub tickets: Vec<TicketKey>,
    /// In the order given.
    pub depends_on: Vec<QuestionRef>,
    /// Who asked, such as a Claude session; `created_by` is the person it asked for.
    pub origin: Id<Actor>,
    pub origin_session: Option<String>,
    pub decision: Option<Decision>,
    pub created_by: Id<Actor>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

/// The checked fields of a new question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionDraft {
    pub title: QuestionTitle,
    pub description: String,
    pub occasion: Option<String>,
    pub tags: Vec<String>,
    pub tickets: Vec<TicketKey>,
    pub depends_on: Vec<QuestionRef>,
    pub origin: Id<Actor>,
    pub origin_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question(QuestionRecord);

impl Question {
    #[must_use]
    pub fn new(
        id: Id<Self>,
        key: QuestionKey,
        project: Id<Project>,
        draft: QuestionDraft,
        created_by: Id<Actor>,
        now: Timestamp,
    ) -> Self {
        Self(QuestionRecord {
            id,
            key,
            project,
            title: draft.title,
            description: draft.description,
            state: QuestionState::Open,
            occasion: draft.occasion,
            tags: draft.tags,
            tickets: draft.tickets,
            depends_on: draft.depends_on,
            origin: draft.origin,
            origin_session: draft.origin_session,
            decision: None,
            created_by,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        })
    }

    #[must_use]
    pub const fn restore(record: QuestionRecord) -> Self {
        Self(record)
    }

    #[must_use]
    pub const fn record(&self) -> &QuestionRecord {
        &self.0
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.0.id
    }

    #[must_use]
    pub const fn key(&self) -> &QuestionKey {
        &self.0.key
    }

    #[must_use]
    pub const fn state(&self) -> QuestionState {
        self.0.state
    }

    /// The next version with nothing else changed, as a comment leaves it.
    #[must_use]
    pub fn touched(&self, now: Timestamp) -> Self {
        self.with(self.0.state, self.0.decision.clone(), now)
    }

    fn with(&self, state: QuestionState, decision: Option<Decision>, now: Timestamp) -> Self {
        Self(QuestionRecord {
            state,
            decision,
            updated_at: now,
            version: self.0.version.next(),
            ..self.0.clone()
        })
    }

    fn require_open(&self) -> Result<(), DecisionError> {
        if self.0.state == QuestionState::Open {
            Ok(())
        } else {
            Err(DecisionError::NotOpen(self.0.key.clone()))
        }
    }

    /// Answers by an option of the `newest` round or an own text; only a person may.
    pub fn answered(
        &self,
        input: AnswerInput<'_>,
        newest: &[QuestionOption],
        by: &Actor,
        now: Timestamp,
    ) -> Result<Self, DecisionError> {
        // the prefix alone proves nothing; only the dispatcher may answer as autopilot
        let autopilot = input.by_autopilot() && by.kind() == ActorKind::Dispatcher;
        if !autopilot {
            require_person(by)?;
        }
        if by.kind() != ActorKind::Dispatcher
            && trimmed(input.reason).is_some_and(|reason| reason.starts_with(AUTOPILOT_PREFIX))
        {
            return Err(DecisionError::AutopilotReasonReserved);
        }
        self.require_open()?;
        let choice = match (trimmed(input.option), trimmed(input.text)) {
            (Some(option), None) => {
                let number = option_number(option)
                    .filter(|number| newest.iter().any(|known| known.number == *number))
                    .ok_or_else(|| DecisionError::UnknownOption(option.to_owned()))?;
                Choice::Option(number)
            }
            (None, Some(text)) => Choice::Text(text.to_owned()),
            _ => return Err(DecisionError::Ambiguous),
        };
        let decision = Decision {
            choice: Some(choice),
            reason: trimmed(input.reason).map(str::to_owned),
            by: by.id(),
            at: now,
        };
        Ok(self.with(QuestionState::Answered, Some(decision), now))
    }

    /// Discards an open question; the reason is required.
    pub fn discarded(
        &self,
        reason: Option<&str>,
        by: &Actor,
        now: Timestamp,
    ) -> Result<Self, DecisionError> {
        require_person(by)?;
        self.require_open()?;
        let reason = trimmed(reason).ok_or(DecisionError::ReasonMissing)?;
        let decision = Decision {
            choice: None,
            reason: Some(reason.to_owned()),
            by: by.id(),
            at: now,
        };
        Ok(self.with(QuestionState::Discarded, Some(decision), now))
    }

    /// Opens an answered or discarded question again and forgets the decision.
    pub fn reopened(&self, by: &Actor, now: Timestamp) -> Result<Self, DecisionError> {
        require_person(by)?;
        if self.0.state == QuestionState::Open {
            return Err(DecisionError::AlreadyOpen(self.0.key.clone()));
        }
        Ok(self.with(QuestionState::Open, None, now))
    }
}

fn require_person(by: &Actor) -> Result<(), DecisionError> {
    if by.kind() == ActorKind::Human {
        Ok(())
    } else {
        Err(DecisionError::HumanOnly)
    }
}

/// The note on each ticket a decision touches, such as `question answered: ARC-Q0001 "Why?"`.
#[must_use]
pub fn ticket_note(
    step: &str,
    key: &QuestionKey,
    title: &QuestionTitle,
    reason: Option<&str>,
) -> String {
    let because = trimmed(reason).map_or_else(String::new, |reason| format!("; reason: {reason}"));
    let note = format!("{step}: {key} \"{}\"{because}", title.as_str());
    if utf16_len(&note) > TICKET_NOTE_MAX {
        format!("{}...", utf16_prefix(&note, TICKET_NOTE_MAX - 3))
    } else {
        note
    }
}

/// A decision in words, such as `option O2 "Db" chosen. Reason: fits`.
#[must_use]
pub fn describe(state: QuestionState, decision: &Decision, options: &[QuestionOption]) -> String {
    let because = decision
        .reason
        .as_deref()
        .map_or_else(String::new, |reason| format!(" Reason: {reason}"));
    match (&decision.choice, state) {
        (_, QuestionState::Discarded) | (None, _) => format!("discarded.{because}"),
        (Some(Choice::Option(number)), _) => {
            let title = options
                .iter()
                .find(|option| option.number == *number)
                .map_or_else(String::new, |option| format!(" \"{}\"", option.title));
            format!("option O{number}{title} chosen.{because}")
        }
        (Some(Choice::Text(text)), _) => format!("answer: {text}.{because}"),
    }
}

/// What a reopening keeps on the question, as legacy wrote it into the question's history.
#[must_use]
pub fn reopen_note(reason: Option<&str>, earlier: Option<(&str, &str)>) -> String {
    let mut parts = vec![String::from("Reopened.")];
    if let Some(reason) = trimmed(reason) {
        parts.push(format!("Reason: {reason}."));
    }
    if let Some((decision, by)) = earlier {
        parts.push(format!("Earlier decision: {decision} ({by})"));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ActorName;

    fn id<T>(value: u128) -> Id<T> {
        Id::from_uuid(uuid::Uuid::from_u128(value))
    }

    fn actor(kind: ActorKind) -> Actor {
        Actor::new(
            id(9),
            kind,
            ActorName::parse("anna").unwrap(),
            Timestamp::from_millis(0),
        )
    }

    fn draft(title: &str) -> OptionDraft {
        OptionDraft {
            title: title.to_owned(),
            text: format!("{title} text"),
            ..OptionDraft::default()
        }
    }

    fn question() -> Question {
        let draft = QuestionDraft {
            title: QuestionTitle::parse("Which store?").unwrap(),
            description: String::new(),
            occasion: None,
            tags: Vec::new(),
            tickets: Vec::new(),
            depends_on: Vec::new(),
            origin: id(1),
            origin_session: None,
        };
        let key = QuestionKey::parse("ARC-Q1").unwrap();
        Question::new(id(2), key, id(3), draft, id(1), Timestamp::from_millis(5))
    }

    fn options() -> Vec<QuestionOption> {
        number_options(&[draft("File"), draft("Db")], &[]).unwrap()
    }

    #[test]
    fn question_keys_read_loosely_and_print_padded() {
        let key = QuestionKey::parse(" arc-q7 ").unwrap();
        assert_eq!(key.to_string(), "ARC-Q0007");
        assert_eq!(QuestionKey::parse("ARC-Q12345").unwrap().number(), 12345);
        assert_eq!(key.project().as_str(), "ARC");
        for bad in [
            "ARC-0007",
            "ARC-F0001",
            "ARC-Q",
            "Q7",
            "1RC-Q1",
            "ARC-Qx",
            "ARC-Q-1",
        ] {
            assert_eq!(
                QuestionKey::parse(bad),
                Err(QuestionError::InvalidKey(bad.to_owned())),
                "{bad}"
            );
        }
    }

    #[test]
    fn references_are_questions_or_tickets() {
        assert_eq!(
            QuestionRef::parse("arc-q2").unwrap().to_string(),
            "ARC-Q0002"
        );
        assert_eq!(QuestionRef::parse("arc-3").unwrap().to_string(), "ARC-0003");
        assert_eq!(
            QuestionRef::parse("nonsense"),
            Err(QuestionError::InvalidReference(String::from("nonsense")))
        );
    }

    #[test]
    fn states_read_their_english_names_only() {
        assert_eq!(
            QuestionState::parse("answered"),
            Ok(QuestionState::Answered)
        );
        assert_eq!(QuestionState::Discarded.as_str(), "discarded");
        assert_eq!(
            QuestionState::parse("verworfen"),
            Err(QuestionError::InvalidState(String::from("verworfen")))
        );
    }

    #[test]
    fn titles_occasions_and_sessions_are_trimmed_and_bounded() {
        assert_eq!(QuestionTitle::parse(" Why? ").unwrap().as_str(), "Why?");
        assert_eq!(
            QuestionTitle::parse(" "),
            Err(QuestionError::Title(TextError::Empty))
        );
        assert_eq!(
            QuestionTitle::parse(&"x".repeat(401)),
            Err(QuestionError::Title(TextError::TooLong(400)))
        );
        assert_eq!(occasion("  "), Ok(None));
        assert_eq!(occasion(" planning ").unwrap().as_deref(), Some("planning"));
        assert_eq!(
            session(&"s".repeat(101)),
            Err(QuestionError::Session(TextError::TooLong(100)))
        );
    }

    #[test]
    fn options_count_on_across_rounds_and_keep_only_real_pros() {
        let first = options();
        assert_eq!(
            first.iter().map(QuestionOption::key).collect::<Vec<_>>(),
            ["O1", "O2"]
        );
        let earlier = Research {
            id: id(1),
            question: id(2),
            actor: id(3),
            context: String::new(),
            options: first,
            created_at: Timestamp::from_millis(0),
        };
        let mut next = draft(" Cloud ");
        next.pros = vec![String::from(" fast "), String::from(" ")];
        next.recommended = true;
        let second = number_options(&[next], &[earlier]).unwrap();
        assert_eq!(second[0].number, 3);
        assert_eq!(second[0].title, "Cloud");
        assert_eq!(second[0].pros, ["fast"]);
        assert!(second[0].recommended);
    }

    #[test]
    fn option_rounds_follow_the_legacy_limits_in_order() {
        let six: Vec<OptionDraft> = (0..6).map(|n| draft(&n.to_string())).collect();
        assert_eq!(
            number_options(&six, &[]),
            Err(ResearchError::TooManyOptions)
        );
        let mut both = vec![draft("A"), draft("B")];
        for option in &mut both {
            option.recommended = true;
        }
        assert_eq!(
            number_options(&both, &[]),
            Err(ResearchError::RecommendedMultiple)
        );
        let blank = OptionDraft {
            title: String::from("A"),
            text: String::from(" "),
            ..OptionDraft::default()
        };
        assert_eq!(
            number_options(&[blank], &[]),
            Err(ResearchError::OptionIncomplete)
        );
        assert_eq!(
            number_options(&[draft(&"t".repeat(401))], &[]),
            Err(ResearchError::OptionTitleTooLong)
        );
    }

    #[test]
    fn a_person_answers_by_an_option_of_the_newest_round_or_a_text() {
        let person = actor(ActorKind::Human);
        let now = Timestamp::from_millis(9);
        let input = AnswerInput {
            option: Some(" o2 "),
            reason: Some(" fits "),
            ..AnswerInput::default()
        };
        let answered = question()
            .answered(input, &options(), &person, now)
            .unwrap();
        let record = answered.record();
        assert_eq!(record.state, QuestionState::Answered);
        assert_eq!(record.version, Version::INITIAL.next());
        let decision = record.decision.as_ref().unwrap();
        assert_eq!(decision.choice, Some(Choice::Option(2)));
        assert_eq!(decision.reason.as_deref(), Some("fits"));
        assert_eq!((decision.by, decision.at), (person.id(), now));
        let text = AnswerInput {
            text: Some("Neither"),
            ..AnswerInput::default()
        };
        let own = question().answered(text, &[], &person, now).unwrap();
        let decision = own.record().decision.as_ref().unwrap();
        assert_eq!(decision.choice, Some(Choice::Text(String::from("Neither"))));
        assert_eq!(decision.reason, None);
    }

    #[test]
    fn answers_are_refused_in_the_legacy_order() {
        let person = actor(ActorKind::Human);
        let agent = actor(ActorKind::Session);
        let now = Timestamp::from_millis(9);
        let pick = |option| AnswerInput {
            option: Some(option),
            ..AnswerInput::default()
        };
        assert_eq!(
            question().answered(pick("O1"), &options(), &agent, now),
            Err(DecisionError::HumanOnly)
        );
        let answered = question()
            .answered(pick("O1"), &options(), &person, now)
            .unwrap();
        assert!(matches!(
            answered.answered(pick("O1"), &options(), &person, now),
            Err(DecisionError::NotOpen(_))
        ));
        let both = AnswerInput {
            option: Some("O1"),
            text: Some("x"),
            reason: None,
        };
        assert_eq!(
            question().answered(both, &options(), &person, now),
            Err(DecisionError::Ambiguous)
        );
        assert_eq!(
            question().answered(AnswerInput::default(), &options(), &person, now),
            Err(DecisionError::Ambiguous)
        );
        for unknown in ["O3", "first", "O"] {
            assert_eq!(
                question().answered(pick(unknown), &options(), &person, now),
                Err(DecisionError::UnknownOption(unknown.to_owned()))
            );
        }
    }

    #[test]
    fn only_the_dispatcher_may_answer_with_the_autopilot_prefix() {
        let now = Timestamp::from_millis(9);
        let taken = AnswerInput {
            option: Some("O1"),
            text: None,
            reason: Some("Autopilot: recommended by the research"),
        };
        assert_eq!(
            question().answered(taken, &options(), &actor(ActorKind::Session), now),
            Err(DecisionError::HumanOnly)
        );
        assert_eq!(
            question().answered(taken, &options(), &actor(ActorKind::Subagent), now),
            Err(DecisionError::HumanOnly)
        );
        assert_eq!(
            question().answered(taken, &options(), &actor(ActorKind::Human), now),
            Err(DecisionError::AutopilotReasonReserved)
        );
    }

    #[test]
    fn the_autopilot_may_take_an_option_with_its_prefix_only() {
        let dispatcher = actor(ActorKind::Dispatcher);
        let now = Timestamp::from_millis(9);
        let taken = AnswerInput {
            option: Some("O1"),
            text: None,
            reason: Some("Autopilot: recommended by the research"),
        };
        let answered = question()
            .answered(taken, &options(), &dispatcher, now)
            .unwrap();
        assert!(answered.record().decision.as_ref().unwrap().by_autopilot());
        let guessed = AnswerInput {
            option: None,
            text: Some("own"),
            reason: Some("Autopilot: guessed"),
        };
        assert_eq!(
            question().answered(guessed, &options(), &dispatcher, now),
            Err(DecisionError::HumanOnly)
        );
    }

    #[test]
    fn discarding_needs_a_person_and_a_reason_and_reopening_clears_the_decision() {
        let person = actor(ActorKind::Human);
        let now = Timestamp::from_millis(9);
        assert_eq!(
            question().discarded(Some(" "), &person, now),
            Err(DecisionError::ReasonMissing)
        );
        assert_eq!(
            question().discarded(Some("x"), &actor(ActorKind::Session), now),
            Err(DecisionError::HumanOnly)
        );
        let discarded = question().discarded(Some(" moot "), &person, now).unwrap();
        assert_eq!(discarded.state(), QuestionState::Discarded);
        let decision = discarded.record().decision.as_ref().unwrap();
        assert_eq!(
            (decision.choice.as_ref(), decision.reason.as_deref()),
            (None, Some("moot"))
        );
        assert!(matches!(
            question().reopened(&person, now),
            Err(DecisionError::AlreadyOpen(_))
        ));
        let reopened = discarded.reopened(&person, now).unwrap();
        assert_eq!(reopened.state(), QuestionState::Open);
        assert_eq!(reopened.record().decision, None);
        assert_eq!(reopened.record().version.value(), 3);
        assert_eq!(question().touched(now).record().version.value(), 2);
    }

    #[test]
    fn decisions_and_reopenings_read_like_legacy_history() {
        let decision = |choice, reason: Option<&str>| Decision {
            choice,
            reason: reason.map(str::to_owned),
            by: id(1),
            at: Timestamp::from_millis(0),
        };
        let chosen = decision(Some(Choice::Option(2)), Some("fits"));
        assert_eq!(
            describe(QuestionState::Answered, &chosen, &options()),
            "option O2 \"Db\" chosen. Reason: fits"
        );
        let own = decision(Some(Choice::Text(String::from("Neither"))), None);
        assert_eq!(
            describe(QuestionState::Answered, &own, &[]),
            "answer: Neither."
        );
        let gone = decision(None, Some("moot"));
        assert_eq!(
            describe(QuestionState::Discarded, &gone, &[]),
            "discarded. Reason: moot"
        );
        assert_eq!(
            reopen_note(
                Some(" new facts "),
                Some(("discarded. Reason: moot", "anna"))
            ),
            "Reopened. Reason: new facts. Earlier decision: discarded. Reason: moot (anna)"
        );
        assert_eq!(reopen_note(None, None), "Reopened.");
    }

    #[test]
    fn ticket_notes_are_clipped_to_the_legacy_room() {
        let key = QuestionKey::parse("ARC-Q1").unwrap();
        let short = QuestionTitle::parse("Which store?").unwrap();
        assert_eq!(
            ticket_note("question answered", &key, &short, Some(" fits ")),
            "question answered: ARC-Q0001 \"Which store?\"; reason: fits"
        );
        assert_eq!(
            ticket_note("question reopened", &key, &short, Some(" ")),
            "question reopened: ARC-Q0001 \"Which store?\""
        );
        let long = QuestionTitle::parse(&"x".repeat(400)).unwrap();
        let note = ticket_note("question answered", &key, &long, None);
        assert_eq!(utf16_len(&note), 200);
        assert!(note.ends_with("..."));
    }
}
