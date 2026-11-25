//! Questions on a project: asked with options, answered by a person, holding tickets back.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Actor, ActorKind, AnswerInput, Comment, CommentBody, CommentOn, DecisionError, HistoryEntry,
    Id, OptionDraft, Project, Question, QuestionDraft, QuestionError, QuestionKey, QuestionOption,
    QuestionRef, QuestionState, QuestionTitle, Research, ResearchError, RuleError, Ticket,
    TicketKey, TicketStatus, Timestamp, Version, describe, normalize_labels, number_options,
    occasion, reopen_note, session, ticket_note,
};

use crate::authors::{self, Author};
use crate::boards::ActorRepository;
use crate::comments;
use crate::projects::{self, ProjectRepository};
use crate::tickets::{self, TicketRepository, TicketWrite};
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

/// An open question as the tickets it holds back show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenQuestion {
    pub key: QuestionKey,
    pub title: String,
}

/// A ticket the question is about, with what lists show of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionTicket {
    pub key: TicketKey,
    pub title: String,
    pub status: TicketStatus,
}

/// Where a dependency of a question stands now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyState {
    Question(QuestionState),
    Ticket(TicketStatus),
}

impl DependencyState {
    /// An open question or a ticket that is neither done nor not needed holds the answer back.
    #[must_use]
    pub const fn open(self) -> bool {
        match self {
            Self::Question(state) => matches!(state, QuestionState::Open),
            Self::Ticket(status) => status.is_open(),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Question(state) => state.as_str(),
            Self::Ticket(status) => status.as_str(),
        }
    }
}

/// A question with the names and states lists need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionRow {
    pub question: Question,
    pub origin_name: String,
    pub origin_kind: ActorKind,
    pub decided_by_name: Option<String>,
    pub tickets: Vec<QuestionTicket>,
    /// Every dependency in the order given.
    pub depends: Vec<(QuestionRef, DependencyState)>,
    pub research_rounds: u32,
    /// Options of the newest round.
    pub option_count: u32,
}

/// A question as lists show it: what it waits for and whether a person can decide it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionView {
    pub row: QuestionRow,
    pub waiting_on: Vec<(QuestionRef, DependencyState)>,
    pub ready: bool,
}

impl QuestionView {
    #[must_use]
    pub fn new(row: QuestionRow) -> Self {
        let waiting_on: Vec<_> = row
            .depends
            .iter()
            .filter(|(_, state)| state.open())
            .cloned()
            .collect();
        let ready = row.question.state() == QuestionState::Open && waiting_on.is_empty();
        Self {
            row,
            waiting_on,
            ready,
        }
    }

    fn order(&self) -> (bool, Timestamp, u32) {
        let record = self.row.question.record();
        (!self.ready, record.created_at, record.key.number())
    }
}

/// A comment on a question with its author.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionComment {
    pub comment: Comment,
    pub question_key: QuestionKey,
    pub author_name: String,
    pub author_kind: ActorKind,
}

/// The question with its rounds of options and comments, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionRead {
    pub row: QuestionRow,
    pub research: Vec<Research>,
    pub comments: Vec<QuestionComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionDetail {
    pub view: QuestionView,
    pub research: Vec<Research>,
    pub comments: Vec<QuestionComment>,
}

impl QuestionDetail {
    /// Options of the newest round, the only ones an answer may choose.
    #[must_use]
    pub fn newest_options(&self) -> &[QuestionOption] {
        self.research
            .last()
            .map_or(&[], |round| round.options.as_slice())
    }
}

/// How many questions of the project are in each state, before any filter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QuestionCounts {
    pub open: u32,
    pub answered: u32,
    pub discarded: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionList {
    pub questions: Vec<QuestionView>,
    pub counts: QuestionCounts,
}

/// Question changes inside a ticket write, so tickets and questions commit together.
#[async_trait]
pub trait QuestionWrite: Send {
    /// The next question number of the project; never handed out twice.
    async fn take_question_number(&mut self, project: Id<Project>) -> Result<u32, AppError>;
    /// The question with this key, read for a change.
    async fn question(&mut self, key: &QuestionKey) -> Result<Option<Question>, AppError>;
    /// Stores the question with its tags, tickets and dependencies.
    async fn insert_question(&mut self, question: &Question) -> Result<Change, AppError>;
    /// Stores state, decision and version if still at `expected`; the lists stay.
    async fn update_question(
        &mut self,
        question: &Question,
        expected: Version,
    ) -> Result<Change, AppError>;
    async fn insert_research(&mut self, research: &Research) -> Result<(), AppError>;
    /// Rounds of the question, oldest first.
    async fn research(&mut self, question: Id<Question>) -> Result<Vec<Research>, AppError>;
    /// Open questions about the ticket, by key.
    async fn open_questions(&mut self, ticket: Id<Ticket>) -> Result<Vec<OpenQuestion>, AppError>;
    /// Questions about the ticket or depending on it.
    async fn questions_naming(&mut self, ticket: Id<Ticket>) -> Result<Vec<Question>, AppError>;
    /// History lines on tickets without a new ticket version, as legacy wrote them.
    async fn note_tickets(&mut self, history: &[HistoryEntry]) -> Result<(), AppError>;
}

#[async_trait]
pub trait QuestionRepository: Send + Sync {
    /// Questions of the project in one snapshot; `None` when the project does not exist.
    async fn list(&self, project: Id<Project>) -> Result<Option<Vec<QuestionRow>>, AppError>;
    async fn get(&self, key: &QuestionKey) -> Result<Option<QuestionRead>, AppError>;
    async fn exists(&self, key: &QuestionKey) -> Result<bool, AppError>;
}

/// Input of a new question; blank entries of the lists are skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewQuestion {
    pub title: String,
    pub description: Option<String>,
    pub occasion: Option<String>,
    pub tags: Vec<String>,
    /// Keys of tickets of the project the question is about, such as `arc-7`.
    pub tickets: Vec<String>,
    /// Questions or tickets the answer waits for.
    pub depends_on: Vec<String>,
    /// The first round of options; none leaves the question without one.
    pub options: Vec<OptionDraft>,
}

/// Which questions a list keeps; every given criterion must hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuestionFilter {
    /// `None` keeps every state.
    pub state: Option<QuestionState>,
    pub ticket: Option<TicketKey>,
    pub tag: Option<String>,
    /// Folded text in key, title, description, occasion, tags or ticket keys.
    pub text: Option<String>,
}

impl QuestionFilter {
    /// Reads the filter as typed; a blank state or `all` keeps every state.
    pub fn parse(
        state: Option<&str>,
        ticket: Option<&str>,
        tag: Option<&str>,
        text: Option<&str>,
    ) -> Result<Self, AppError> {
        let state = match state.map(str::trim) {
            None | Some("" | "all") => None,
            Some(text) => Some(QuestionState::parse(text).map_err(|error| invalid(&error))?),
        };
        let ticket = ticket
            .filter(|text| !text.trim().is_empty())
            .map(TicketKey::parse)
            .transpose()
            .map_err(|error| tickets::invalid(&error))?;
        let blank = |text: Option<&str>| {
            text.map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        Ok(Self {
            state,
            ticket,
            tag: blank(tag),
            text: blank(text).map(|text| text.to_lowercase()),
        })
    }

    fn accepts(&self, view: &QuestionView) -> bool {
        let record = view.row.question.record();
        self.state.is_none_or(|state| record.state == state)
            && self
                .ticket
                .as_ref()
                .is_none_or(|ticket| record.tickets.contains(ticket))
            && self
                .tag
                .as_ref()
                .is_none_or(|tag| record.tags.contains(tag))
            && self.text.as_deref().is_none_or(|text| {
                let fields = [
                    record.key.to_string(),
                    record.title.as_str().to_owned(),
                    record.description.clone(),
                    record.occasion.clone().unwrap_or_default(),
                ];
                fields
                    .into_iter()
                    .chain(record.tags.iter().cloned())
                    .chain(record.tickets.iter().map(ToString::to_string))
                    .any(|field| field.to_lowercase().contains(text))
            })
    }
}

/// An answer as typed; exactly one of option and text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    pub option: Option<String>,
    pub text: Option<String>,
    pub reason: Option<String>,
}

enum Step<'a> {
    Answer(&'a Answer),
    Discard(Option<&'a str>),
    Reopen(Option<&'a str>),
}

impl Step<'_> {
    fn reason(&self) -> Option<&str> {
        match self {
            Self::Answer(answer) => answer.reason.as_deref(),
            Self::Discard(reason) | Self::Reopen(reason) => *reason,
        }
    }

    /// The note on each ticket of the question, as legacy wrote it.
    const fn label(&self) -> &'static str {
        match self {
            Self::Answer(_) => "question answered",
            Self::Discard(_) => "question discarded",
            Self::Reopen(_) => "question reopened",
        }
    }
}

#[must_use]
pub fn invalid(error: &QuestionError) -> AppError {
    let message = error.to_string();
    match error {
        QuestionError::InvalidKey(_) => AppError::invalid("question-id-invalid", message),
        QuestionError::InvalidReference(_) => AppError::invalid("reference-invalid", message),
        QuestionError::InvalidState(_) => AppError::invalid("question-state-invalid", message),
        QuestionError::Title(text) => AppError::text(*text, "title-empty", message),
        QuestionError::Occasion(text) | QuestionError::Session(text) => {
            AppError::text(*text, "value-too-long", message)
        }
    }
}

fn research_invalid(error: ResearchError) -> AppError {
    let code = match error {
        ResearchError::TooManyOptions | ResearchError::OptionIncomplete => "research-invalid",
        ResearchError::RecommendedMultiple => "research-recommended-multiple",
        ResearchError::OptionTitleTooLong => "value-too-long",
    };
    AppError::invalid(code, error.to_string())
}

fn refused(error: &DecisionError) -> AppError {
    let code = match error {
        DecisionError::HumanOnly => "human-only",
        DecisionError::NotOpen(_) | DecisionError::AlreadyOpen(_) => "question-state-invalid",
        DecisionError::Ambiguous => "decision-ambiguous",
        DecisionError::UnknownOption(_) => "decision-option-unknown",
        DecisionError::ReasonMissing => "decision-reason-missing",
        DecisionError::AutopilotReasonReserved => "autopilot-reason-reserved",
    };
    AppError::invalid(code, error.to_string())
}

#[must_use]
pub fn not_found(key: &QuestionKey) -> AppError {
    AppError::not_found(
        "question-not-found",
        format!("question {key} does not exist"),
    )
}

fn ticket_unknown(key: &TicketKey) -> AppError {
    AppError::invalid(
        "question-ticket-unknown",
        format!("ticket {key} does not exist"),
    )
}

fn ticket_other_project(key: &TicketKey) -> AppError {
    AppError::invalid(
        "question-ticket-other-project",
        format!("ticket {key} belongs to another project; a question is about its own tickets"),
    )
}

fn dependency_unknown(reference: &QuestionRef) -> AppError {
    AppError::invalid(
        "dependency-unknown",
        format!("dependency {reference} does not exist"),
    )
}

fn tags_invalid(error: &RuleError) -> AppError {
    AppError::invalid("value-too-long", error.to_string())
}

/// Keys as typed without blanks and repeats; ticket keys sorted, references in the order given.
fn ticket_keys(texts: &[String]) -> Result<Vec<TicketKey>, AppError> {
    let mut keys = texts
        .iter()
        .filter(|text| !text.trim().is_empty())
        .map(|text| TicketKey::parse(text))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| tickets::invalid(&error))?;
    keys.sort();
    keys.dedup();
    Ok(keys)
}

fn references(texts: &[String]) -> Result<Vec<QuestionRef>, AppError> {
    let mut found: Vec<QuestionRef> = Vec::new();
    for text in texts.iter().filter(|text| !text.trim().is_empty()) {
        let reference = QuestionRef::parse(text).map_err(|error| invalid(&error))?;
        if !found.contains(&reference) {
            found.push(reference);
        }
    }
    Ok(found)
}

pub struct QuestionService {
    tickets: Arc<dyn TicketRepository>,
    questions: Arc<dyn QuestionRepository>,
    projects: Arc<dyn ProjectRepository>,
    actors: Arc<dyn ActorRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for QuestionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuestionService").finish_non_exhaustive()
    }
}

impl QuestionService {
    #[must_use]
    pub fn new(
        tickets: Arc<dyn TicketRepository>,
        questions: Arc<dyn QuestionRepository>,
        projects: Arc<dyn ProjectRepository>,
        actors: Arc<dyn ActorRepository>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            tickets,
            questions,
            projects,
            actors,
            clock,
            ids,
            signal,
        }
    }

    /// Asks a question on the project, checking in the legacy order before the number is taken.
    pub async fn create(
        &self,
        project: Id<Project>,
        input: &NewQuestion,
        author: Author<'_>,
        person: &Actor,
    ) -> Result<QuestionDetail, AppError> {
        let origin_session = match author.session.filter(|_| !author.by_person) {
            Some(text) => session(text).map_err(|error| invalid(&error))?,
            None => None,
        };
        let project = self
            .projects
            .get(project)
            .await?
            .ok_or_else(|| projects::not_found(project))?;
        let title = QuestionTitle::parse(&input.title).map_err(|error| invalid(&error))?;
        let tags = normalize_labels(&input.tags).map_err(|error| tags_invalid(&error))?;
        let occasion = match input.occasion.as_deref() {
            Some(text) => occasion(text).map_err(|error| invalid(&error))?,
            None => None,
        };
        let ticket_keys = ticket_keys(&input.tickets)?;
        let depends_on = references(&input.depends_on)?;
        let options = number_options(&input.options, &[]).map_err(research_invalid)?;
        // the actor commits on its own, before the write takes the connection
        let origin = self.author(author, person).await?;
        let mut write = self.tickets.begin().await?;
        check_tickets(write.as_mut(), project.id(), &ticket_keys).await?;
        let number = write.take_question_number(project.id()).await?;
        let key = QuestionKey::new(project.key().clone(), number);
        check_dependencies(write.as_mut(), &depends_on).await?;
        let now = self.clock.now();
        let draft = QuestionDraft {
            title,
            description: input.description.clone().unwrap_or_default(),
            occasion,
            tags,
            tickets: ticket_keys,
            depends_on,
            origin: origin.id(),
            origin_session,
        };
        let question = Question::new(
            self.ids.next(),
            key.clone(),
            project.id(),
            draft,
            person.id(),
            now,
        );
        let change = write.insert_question(&question).await?;
        if !options.is_empty() {
            let research = Research {
                id: self.ids.next(),
                question: question.id(),
                actor: origin.id(),
                context: String::new(),
                options,
                created_at: now,
            };
            write.insert_research(&research).await?;
        }
        write.commit().await?;
        self.signal.notify(&change);
        self.read(&key).await
    }

    /// Questions of the project: ready ones first, oldest first within, with counts per state.
    pub async fn list(
        &self,
        project: Id<Project>,
        filter: &QuestionFilter,
    ) -> Result<QuestionList, AppError> {
        let rows = self
            .questions
            .list(project)
            .await?
            .ok_or_else(|| projects::not_found(project))?;
        let mut counts = QuestionCounts::default();
        let mut questions = Vec::new();
        for row in rows {
            match row.question.state() {
                QuestionState::Open => counts.open += 1,
                QuestionState::Answered => counts.answered += 1,
                QuestionState::Discarded => counts.discarded += 1,
            }
            let view = QuestionView::new(row);
            if filter.accepts(&view) {
                questions.push(view);
            }
        }
        questions.sort_by_key(QuestionView::order);
        Ok(QuestionList { questions, counts })
    }

    /// The question with its options and comments; the key may be typed loosely, such as `arc-q7`.
    pub async fn get(&self, key: &str) -> Result<QuestionDetail, AppError> {
        let key = QuestionKey::parse(key).map_err(|error| invalid(&error))?;
        self.read(&key).await
    }

    async fn read(&self, key: &QuestionKey) -> Result<QuestionDetail, AppError> {
        let read = self
            .questions
            .get(key)
            .await?
            .ok_or_else(|| not_found(key))?;
        Ok(QuestionDetail {
            view: QuestionView::new(read.row),
            research: read.research,
            comments: read.comments,
        })
    }

    /// Appends a comment; the question gets a new version, as in legacy.
    pub async fn comment(
        &self,
        key: &str,
        body: &str,
        author: Author<'_>,
        person: &Actor,
    ) -> Result<QuestionComment, AppError> {
        let key = QuestionKey::parse(key).map_err(|error| invalid(&error))?;
        let body = CommentBody::parse(body).map_err(|error| comments::invalid(&error))?;
        if !self.questions.exists(&key).await? {
            return Err(not_found(&key));
        }
        let actor = self.author(author, person).await?;
        let mut write = self.tickets.begin().await?;
        let before = load(write.as_mut(), &key, None).await?;
        let now = self.clock.now();
        let comment = Comment::new(
            self.ids.next(),
            CommentOn::Question(before.id()),
            actor.id(),
            body,
            author.in_session(),
            now,
        );
        write
            .update_question(&before.touched(now), before.record().version)
            .await?;
        let change = write.insert_comment(&comment).await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(QuestionComment {
            comment,
            question_key: key,
            author_name: actor.name().as_str().to_owned(),
            author_kind: actor.kind(),
        })
    }

    /// Answers an open question by an option of the newest round or an own text.
    pub async fn answer(
        &self,
        key: &str,
        answer: &Answer,
        author: Author<'_>,
        person: &Actor,
        expected: Option<Version>,
    ) -> Result<QuestionDetail, AppError> {
        self.decide(key, Step::Answer(answer), author, person, expected)
            .await
    }

    /// Discards an open question; the reason is required.
    pub async fn discard(
        &self,
        key: &str,
        reason: Option<&str>,
        author: Author<'_>,
        person: &Actor,
        expected: Option<Version>,
    ) -> Result<QuestionDetail, AppError> {
        self.decide(key, Step::Discard(reason), author, person, expected)
            .await
    }

    /// Opens an answered or discarded question again; a comment keeps the earlier decision.
    pub async fn reopen(
        &self,
        key: &str,
        reason: Option<&str>,
        author: Author<'_>,
        person: &Actor,
        expected: Option<Version>,
    ) -> Result<QuestionDetail, AppError> {
        self.decide(key, Step::Reopen(reason), author, person, expected)
            .await
    }

    async fn decide(
        &self,
        key: &str,
        step: Step<'_>,
        author: Author<'_>,
        person: &Actor,
        expected: Option<Version>,
    ) -> Result<QuestionDetail, AppError> {
        let key = QuestionKey::parse(key).map_err(|error| invalid(&error))?;
        // the earlier decision in words, read before the write takes the connection
        let earlier = match step {
            Step::Reopen(_) => earlier_decision(&self.read(&key).await?),
            _ => None,
        };
        let by = self.author(author, person).await?;
        let mut write = self.tickets.begin().await?;
        let before = load(write.as_mut(), &key, expected).await?;
        let now = self.clock.now();
        let after = match step {
            Step::Answer(answer) => {
                let research = write.research(before.id()).await?;
                let newest = research.last().map_or(&[][..], |round| &round.options);
                let input = AnswerInput {
                    option: answer.option.as_deref(),
                    text: answer.text.as_deref(),
                    reason: answer.reason.as_deref(),
                };
                before.answered(input, newest, &by, now)
            }
            Step::Discard(reason) => before.discarded(reason, &by, now),
            Step::Reopen(_) => before.reopened(&by, now),
        }
        .map_err(|error| refused(&error))?;
        let mut change = write
            .update_question(&after, before.record().version)
            .await?;
        let reason = step.reason();
        if let Step::Reopen(_) = step {
            let earlier = earlier
                .as_ref()
                .map(|(text, by)| (text.as_str(), by.as_str()));
            let body = CommentBody::parse(&reopen_note(reason, earlier))
                .map_err(|error| comments::invalid(&error))?;
            let comment = Comment::new(
                self.ids.next(),
                CommentOn::Question(after.id()),
                by.id(),
                body,
                author.in_session(),
                now,
            );
            change = write.insert_comment(&comment).await?;
        }
        self.note_tickets(write.as_mut(), &before, &after, &step, &by, now)
            .await?;
        write.commit().await?;
        self.signal.notify(&change);
        self.read(&key).await
    }

    /// A line in the history of each ticket of the question: its state before and after.
    async fn note_tickets(
        &self,
        write: &mut dyn TicketWrite,
        before: &Question,
        after: &Question,
        step: &Step<'_>,
        by: &Actor,
        now: Timestamp,
    ) -> Result<(), AppError> {
        let record = after.record();
        let note = ticket_note(step.label(), &record.key, &record.title, step.reason());
        let mut history = Vec::new();
        for key in &record.tickets {
            let Some(ticket) = write.ticket(key).await? else {
                continue;
            };
            history.push(HistoryEntry {
                id: self.ids.next(),
                ticket: ticket.id(),
                actor: by.id(),
                at: now,
                field: "question",
                from: Some(before.state().as_str().to_owned()),
                to: Some(after.state().as_str().to_owned()),
                note: Some(note.clone()),
            });
        }
        write.note_tickets(&history).await
    }

    async fn author(&self, author: Author<'_>, person: &Actor) -> Result<Actor, AppError> {
        authors::resolve(
            self.actors.as_ref(),
            self.ids.as_ref(),
            self.clock.as_ref(),
            author,
            person,
        )
        .await
    }
}

/// The decision of a question in words with the name of who made it.
fn earlier_decision(detail: &QuestionDetail) -> Option<(String, String)> {
    let row = &detail.view.row;
    let record = row.question.record();
    let decision = record.decision.as_ref()?;
    let options: Vec<QuestionOption> = detail
        .research
        .iter()
        .flat_map(|round| round.options.iter().cloned())
        .collect();
    let by = row.decided_by_name.clone().unwrap_or_default();
    Some((describe(record.state, decision, &options), by))
}

/// The question with this key, which must still have `expected` when one is given.
async fn load(
    write: &mut dyn TicketWrite,
    key: &QuestionKey,
    expected: Option<Version>,
) -> Result<Question, AppError> {
    let question = write.question(key).await?.ok_or_else(|| not_found(key))?;
    if expected.is_some_and(|version| version != question.record().version) {
        return Err(AppError::version_conflict("question", key));
    }
    Ok(question)
}

/// Every ticket must exist and belong to the question's project.
async fn check_tickets(
    write: &mut dyn TicketWrite,
    project: Id<Project>,
    keys: &[TicketKey],
) -> Result<(), AppError> {
    for key in keys {
        match write.ticket(key).await? {
            None => return Err(ticket_unknown(key)),
            Some(ticket) if ticket.record().project != project => {
                return Err(ticket_other_project(key));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// A new question cannot be a dependency yet, so only unknown targets are refused.
async fn check_dependencies(
    write: &mut dyn TicketWrite,
    references: &[QuestionRef],
) -> Result<(), AppError> {
    for reference in references {
        let known = match reference {
            QuestionRef::Question(key) => write.question(key).await?.is_some(),
            QuestionRef::Ticket(key) => write.ticket(key).await?.is_some(),
        };
        if !known {
            return Err(dependency_unknown(reference));
        }
    }
    Ok(())
}

/// Questions about a deleted ticket lose it and get a new version, as legacy cleaned up.
pub(crate) async fn forget_ticket(
    write: &mut dyn TicketWrite,
    ticket: &Ticket,
    at: Timestamp,
) -> Result<Vec<Change>, AppError> {
    let mut changes = Vec::new();
    for question in write.questions_naming(ticket.id()).await? {
        let touched = question.touched(at);
        changes.push(
            write
                .update_question(&touched, question.record().version)
                .await?,
        );
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use nullradix_domain::TextError;

    use super::*;

    #[test]
    fn maps_question_errors_to_legacy_codes() {
        let key = QuestionKey::parse("ARC-Q1").unwrap();
        let cases = [
            (
                invalid(&QuestionError::InvalidKey(String::new())),
                "question-id-invalid",
            ),
            (
                invalid(&QuestionError::InvalidReference(String::new())),
                "reference-invalid",
            ),
            (
                invalid(&QuestionError::InvalidState(String::new())),
                "question-state-invalid",
            ),
            (
                invalid(&QuestionError::Title(TextError::Empty)),
                "title-empty",
            ),
            (
                invalid(&QuestionError::Occasion(TextError::TooLong(400))),
                "value-too-long",
            ),
            (
                research_invalid(ResearchError::TooManyOptions),
                "research-invalid",
            ),
            (
                research_invalid(ResearchError::RecommendedMultiple),
                "research-recommended-multiple",
            ),
            (refused(&DecisionError::HumanOnly), "human-only"),
            (
                refused(&DecisionError::NotOpen(key.clone())),
                "question-state-invalid",
            ),
            (
                refused(&DecisionError::AlreadyOpen(key.clone())),
                "question-state-invalid",
            ),
            (refused(&DecisionError::Ambiguous), "decision-ambiguous"),
            (
                refused(&DecisionError::UnknownOption(String::from("O9"))),
                "decision-option-unknown",
            ),
            (
                refused(&DecisionError::ReasonMissing),
                "decision-reason-missing",
            ),
            (
                refused(&DecisionError::AutopilotReasonReserved),
                "autopilot-reason-reserved",
            ),
            (not_found(&key), "question-not-found"),
            (
                ticket_unknown(&TicketKey::parse("ARC-1").unwrap()),
                "question-ticket-unknown",
            ),
            (
                ticket_other_project(&TicketKey::parse("ARC-1").unwrap()),
                "question-ticket-other-project",
            ),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code);
        }
    }

    #[test]
    fn filters_read_all_and_blank_as_every_state() {
        assert_eq!(
            QuestionFilter::parse(Some("all"), None, None, None)
                .unwrap()
                .state,
            None
        );
        assert_eq!(
            QuestionFilter::parse(Some(" "), None, None, None)
                .unwrap()
                .state,
            None
        );
        assert_eq!(
            QuestionFilter::parse(Some("open"), Some("arc-7"), Some(" "), Some(" Store ")).unwrap(),
            QuestionFilter {
                state: Some(QuestionState::Open),
                ticket: Some(TicketKey::parse("ARC-7").unwrap()),
                tag: None,
                text: Some(String::from("store")),
            }
        );
        assert_eq!(
            QuestionFilter::parse(Some("offen"), None, None, None)
                .unwrap_err()
                .code(),
            "question-state-invalid"
        );
        assert_eq!(
            QuestionFilter::parse(None, Some("nope"), None, None)
                .unwrap_err()
                .code(),
            "ticket-id-invalid"
        );
    }

    #[test]
    fn ticket_keys_are_sorted_and_references_keep_their_order() {
        let texts = ["arc-2", " ", "ARC-1", "arc-0002"].map(String::from);
        let keys: Vec<String> = ticket_keys(&texts)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(keys, ["ARC-0001", "ARC-0002"]);
        let texts = ["arc-q2", "arc-1", "ARC-Q0002", ""].map(String::from);
        let found: Vec<String> = references(&texts)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(found, ["ARC-Q0002", "ARC-0001"]);
        assert_eq!(
            references(&[String::from("x")]).unwrap_err().code(),
            "reference-invalid"
        );
    }

    #[test]
    fn open_dependencies_keep_a_question_from_being_ready() {
        assert!(DependencyState::Question(QuestionState::Open).open());
        assert!(!DependencyState::Question(QuestionState::Discarded).open());
        assert!(DependencyState::Ticket(TicketStatus::Blocked).open());
        assert!(!DependencyState::Ticket(TicketStatus::NotNeeded).open());
        assert_eq!(
            DependencyState::Ticket(TicketStatus::InProgress).as_str(),
            "in-progress"
        );
    }
}
