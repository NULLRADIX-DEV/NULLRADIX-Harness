use std::collections::HashMap;

use async_trait::async_trait;
use nullradix_app::questions::{
    DependencyState, OpenQuestion, QuestionComment, QuestionRead, QuestionRepository, QuestionRow,
    QuestionTicket, QuestionWrite, not_found,
};
use nullradix_app::{AppError, Change, ChangeOp, Entity};
use nullradix_domain::{
    ActorKind, Choice, Decision, HistoryEntry, Id, Project, ProjectKey, Question, QuestionKey,
    QuestionOption, QuestionRecord, QuestionRef, QuestionState, QuestionTitle, Research, Ticket,
    TicketKey, TicketStatus, Timestamp, Version,
};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::error::{database, missed_guard};
use crate::tickets::{Write, insert_history};
use crate::{Store, StoreError, change_feed, comments};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid {what}"))
}

fn number(value: i64, what: &str) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| corrupt(what))
}

pub(crate) fn question_key(project: &str, value: i64) -> Result<QuestionKey, StoreError> {
    let project = ProjectKey::parse(project).map_err(|_| corrupt("project key"))?;
    Ok(QuestionKey::new(project, number(value, "question number")?))
}

fn ticket_key(project: &str, value: i64) -> Result<TicketKey, StoreError> {
    let project = ProjectKey::parse(project).map_err(|_| corrupt("project key"))?;
    Ok(TicketKey::new(project, number(value, "ticket number")?))
}

fn state(text: &str) -> Result<QuestionState, StoreError> {
    QuestionState::parse(text).map_err(|_| corrupt("question state"))
}

fn ticket_status(text: &str) -> Result<TicketStatus, StoreError> {
    text.parse().map_err(|_| corrupt("ticket status"))
}

/// Which questions a read covers; `None` does not narrow.
#[derive(Clone, Copy, Default)]
struct Selection {
    project: Option<Uuid>,
    question: Option<Uuid>,
}

struct Row {
    id: Uuid,
    project_key: String,
    project_id: Uuid,
    number: i64,
    title: String,
    description: String,
    state: String,
    occasion: Option<String>,
    origin_actor_id: Uuid,
    origin_name: String,
    origin_kind: String,
    origin_session: Option<String>,
    decision_option: Option<String>,
    decision_text: Option<String>,
    decision_reason: Option<String>,
    decided_by: Option<Uuid>,
    decided_by_name: Option<String>,
    decided_at: Option<i64>,
    created_by: Uuid,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

/// The lists of the read questions, by question id.
#[derive(Default)]
struct Lists {
    tags: HashMap<Uuid, Vec<String>>,
    tickets: HashMap<Uuid, Vec<QuestionTicket>>,
    depends: HashMap<Uuid, Vec<(QuestionRef, DependencyState)>>,
    research: HashMap<Uuid, Vec<Research>>,
}

impl Row {
    fn decision(&self) -> Result<Option<Decision>, StoreError> {
        let (Some(by), Some(at)) = (self.decided_by, self.decided_at) else {
            return Ok(None);
        };
        let choice = match (&self.decision_option, &self.decision_text) {
            (Some(option), None) => {
                let value = option
                    .strip_prefix('O')
                    .and_then(|digits| digits.parse().ok())
                    .ok_or_else(|| corrupt("decision option"))?;
                Some(Choice::Option(value))
            }
            (None, Some(text)) => Some(Choice::Text(text.clone())),
            _ => None,
        };
        Ok(Some(Decision {
            choice,
            reason: self.decision_reason.clone(),
            by: Id::from_uuid(by),
            at: Timestamp::from_millis(at),
        }))
    }

    fn into_question_row(self, lists: &mut Lists) -> Result<QuestionRow, StoreError> {
        let decision = self.decision()?;
        let depends = lists.depends.remove(&self.id).unwrap_or_default();
        let tickets = lists.tickets.remove(&self.id).unwrap_or_default();
        let research = lists.research.remove(&self.id).unwrap_or_default();
        let record = QuestionRecord {
            id: Id::from_uuid(self.id),
            key: question_key(&self.project_key, self.number)?,
            project: Id::from_uuid(self.project_id),
            title: QuestionTitle::parse(&self.title).map_err(|_| corrupt("question title"))?,
            description: self.description,
            state: state(&self.state)?,
            occasion: self.occasion,
            tags: lists.tags.remove(&self.id).unwrap_or_default(),
            tickets: tickets.iter().map(|ticket| ticket.key.clone()).collect(),
            depends_on: depends.iter().map(|(target, _)| target.clone()).collect(),
            origin: Id::from_uuid(self.origin_actor_id),
            origin_session: self.origin_session,
            decision,
            created_by: Id::from_uuid(self.created_by),
            created_at: Timestamp::from_millis(self.created_at),
            updated_at: Timestamp::from_millis(self.updated_at),
            version: Version::from_stored(self.version),
        };
        let count = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
        Ok(QuestionRow {
            question: Question::restore(record),
            origin_name: self.origin_name,
            origin_kind: self
                .origin_kind
                .parse::<ActorKind>()
                .map_err(|_| corrupt("actor kind"))?,
            decided_by_name: self.decided_by_name,
            tickets,
            depends,
            research_rounds: count(research.len()),
            option_count: count(research.last().map_or(0, |round| round.options.len())),
        })
    }
}

async fn rows(connection: &mut SqliteConnection, at: Selection) -> Result<Vec<Row>, StoreError> {
    Ok(sqlx::query_as!(
        Row,
        r#"SELECT q.id AS "id: Uuid", p.key AS project_key, q.project_id AS "project_id: Uuid",
                  q.number, q.title, q.description, q.state, q.occasion,
                  q.origin_actor_id AS "origin_actor_id: Uuid", o.name AS origin_name,
                  o.kind AS origin_kind, q.origin_session, q.decision_option, q.decision_text,
                  q.decision_reason, q.decided_by AS "decided_by: Uuid",
                  d.name AS "decided_by_name?", q.decided_at,
                  q.created_by AS "created_by: Uuid", q.created_at, q.updated_at, q.version
           FROM questions q
           JOIN projects p ON p.id = q.project_id
           JOIN actors o ON o.id = q.origin_actor_id
           LEFT JOIN actors d ON d.id = q.decided_by
           WHERE (?1 IS NULL OR q.project_id = ?1) AND (?2 IS NULL OR q.id = ?2)
           ORDER BY q.number"#,
        at.project,
        at.question,
    )
    .fetch_all(connection)
    .await?)
}

async fn tags(
    connection: &mut SqliteConnection,
    at: Selection,
) -> Result<HashMap<Uuid, Vec<String>>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT g.question_id AS "question_id: Uuid", g.tag
           FROM question_tags g
           JOIN questions q ON q.id = g.question_id
           WHERE (?1 IS NULL OR q.project_id = ?1) AND (?2 IS NULL OR q.id = ?2)
           ORDER BY g.question_id, g.position"#,
        at.project,
        at.question,
    )
    .fetch_all(connection)
    .await?;
    let mut tags: HashMap<Uuid, Vec<String>> = HashMap::new();
    for row in rows {
        tags.entry(row.question_id).or_default().push(row.tag);
    }
    Ok(tags)
}

async fn tickets(
    connection: &mut SqliteConnection,
    at: Selection,
) -> Result<HashMap<Uuid, Vec<QuestionTicket>>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT l.question_id AS "question_id: Uuid", p.key AS project_key, t.number, t.title,
                  t.status
           FROM question_tickets l
           JOIN questions q ON q.id = l.question_id
           JOIN tickets t ON t.id = l.ticket_id
           JOIN projects p ON p.id = t.project_id
           WHERE (?1 IS NULL OR q.project_id = ?1) AND (?2 IS NULL OR q.id = ?2)
           ORDER BY l.question_id, p.key, t.number"#,
        at.project,
        at.question,
    )
    .fetch_all(connection)
    .await?;
    let mut tickets: HashMap<Uuid, Vec<QuestionTicket>> = HashMap::new();
    for row in rows {
        tickets
            .entry(row.question_id)
            .or_default()
            .push(QuestionTicket {
                key: ticket_key(&row.project_key, row.number)?,
                title: row.title,
                status: ticket_status(&row.status)?,
            });
    }
    Ok(tickets)
}

async fn depends(
    connection: &mut SqliteConnection,
    at: Selection,
) -> Result<HashMap<Uuid, Vec<(QuestionRef, DependencyState)>>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT d.question_id AS "question_id: Uuid",
                  qp.key AS "question_project?", dq.number AS "question_number?",
                  dq.state AS "question_state?", tp.key AS "ticket_project?",
                  dt.number AS "ticket_number?", dt.status AS "ticket_status?"
           FROM question_dependencies d
           JOIN questions q ON q.id = d.question_id
           LEFT JOIN questions dq ON dq.id = d.depends_on_question_id
           LEFT JOIN projects qp ON qp.id = dq.project_id
           LEFT JOIN tickets dt ON dt.id = d.depends_on_ticket_id
           LEFT JOIN projects tp ON tp.id = dt.project_id
           WHERE (?1 IS NULL OR q.project_id = ?1) AND (?2 IS NULL OR q.id = ?2)
           ORDER BY d.question_id, d.position"#,
        at.project,
        at.question,
    )
    .fetch_all(connection)
    .await?;
    let mut depends: HashMap<Uuid, Vec<(QuestionRef, DependencyState)>> = HashMap::new();
    for row in rows {
        let entry = match (
            (
                row.question_project,
                row.question_number,
                row.question_state,
            ),
            (row.ticket_project, row.ticket_number, row.ticket_status),
        ) {
            ((Some(project), Some(value), Some(found)), _) => (
                QuestionRef::Question(question_key(&project, value)?),
                DependencyState::Question(state(&found)?),
            ),
            (_, (Some(project), Some(value), Some(status))) => (
                QuestionRef::Ticket(ticket_key(&project, value)?),
                DependencyState::Ticket(ticket_status(&status)?),
            ),
            _ => return Err(corrupt("question dependency")),
        };
        depends.entry(row.question_id).or_default().push(entry);
    }
    Ok(depends)
}

fn texts(json: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(json).map_err(|_| corrupt("option pros or cons"))
}

/// Rounds of the read questions, oldest first, each with its options by number.
async fn research(
    connection: &mut SqliteConnection,
    at: Selection,
) -> Result<HashMap<Uuid, Vec<Research>>, StoreError> {
    let rounds = sqlx::query!(
        r#"SELECT r.id AS "id: Uuid", r.question_id AS "question_id: Uuid",
                  r.actor_id AS "actor_id: Uuid", r.context, r.created_at
           FROM question_research r
           JOIN questions q ON q.id = r.question_id
           WHERE (?1 IS NULL OR q.project_id = ?1) AND (?2 IS NULL OR q.id = ?2)
           ORDER BY r.question_id, r.created_at, r.rowid"#,
        at.project,
        at.question,
    )
    .fetch_all(&mut *connection)
    .await?;
    let options = sqlx::query!(
        r#"SELECT o.research_id AS "research_id: Uuid", o.number, o.title, o.text, o.pros,
                  o.cons, o.recommended
           FROM question_options o
           JOIN questions q ON q.id = o.question_id
           WHERE (?1 IS NULL OR q.project_id = ?1) AND (?2 IS NULL OR q.id = ?2)
           ORDER BY o.number"#,
        at.project,
        at.question,
    )
    .fetch_all(connection)
    .await?;
    let mut by_round: HashMap<Uuid, Vec<QuestionOption>> = HashMap::new();
    for option in options {
        by_round
            .entry(option.research_id)
            .or_default()
            .push(QuestionOption {
                number: number(option.number, "option number")?,
                title: option.title,
                text: option.text,
                pros: texts(&option.pros)?,
                cons: texts(&option.cons)?,
                recommended: option.recommended != 0,
            });
    }
    let mut research: HashMap<Uuid, Vec<Research>> = HashMap::new();
    for round in rounds {
        research
            .entry(round.question_id)
            .or_default()
            .push(Research {
                id: Id::from_uuid(round.id),
                question: Id::from_uuid(round.question_id),
                actor: Id::from_uuid(round.actor_id),
                context: round.context,
                options: by_round.remove(&round.id).unwrap_or_default(),
                created_at: Timestamp::from_millis(round.created_at),
            });
    }
    Ok(research)
}

async fn question_rows(
    connection: &mut SqliteConnection,
    at: Selection,
) -> Result<(Vec<QuestionRow>, HashMap<Uuid, Vec<Research>>), StoreError> {
    let rows = rows(&mut *connection, at).await?;
    let mut lists = Lists {
        tags: tags(&mut *connection, at).await?,
        tickets: tickets(&mut *connection, at).await?,
        depends: depends(&mut *connection, at).await?,
        research: research(connection, at).await?,
    };
    let rounds = lists.research.clone();
    let rows = rows
        .into_iter()
        .map(|row| row.into_question_row(&mut lists))
        .collect::<Result<_, _>>()?;
    Ok((rows, rounds))
}

async fn question_id(
    connection: &mut SqliteConnection,
    key: &QuestionKey,
) -> Result<Option<Uuid>, StoreError> {
    let project = key.project().as_str();
    let number = i64::from(key.number());
    Ok(sqlx::query_scalar!(
        r#"SELECT q.id AS "id: Uuid" FROM questions q JOIN projects p ON p.id = q.project_id
           WHERE p.key = ? AND q.number = ?"#,
        project,
        number,
    )
    .fetch_optional(connection)
    .await?)
}

/// Open questions about the read tickets, by ticket id and then key.
pub(crate) async fn open_by_ticket(
    connection: &mut SqliteConnection,
    project: Option<Uuid>,
    board: Option<Uuid>,
    ticket: Option<Uuid>,
) -> Result<HashMap<Uuid, Vec<OpenQuestion>>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT l.ticket_id AS "ticket_id: Uuid", p.key AS project_key, q.number, q.title
           FROM question_tickets l
           JOIN questions q ON q.id = l.question_id
           JOIN projects p ON p.id = q.project_id
           JOIN tickets t ON t.id = l.ticket_id
           WHERE q.state = 'open'
             AND (?1 IS NULL OR t.project_id = ?1) AND (?2 IS NULL OR t.board_id = ?2)
             AND (?3 IS NULL OR t.id = ?3)
           ORDER BY l.ticket_id, p.key, q.number"#,
        project,
        board,
        ticket,
    )
    .fetch_all(connection)
    .await?;
    let mut open: HashMap<Uuid, Vec<OpenQuestion>> = HashMap::new();
    for row in rows {
        open.entry(row.ticket_id).or_default().push(OpenQuestion {
            key: question_key(&row.project_key, row.number)?,
            title: row.title,
        });
    }
    Ok(open)
}

async fn one(connection: &mut SqliteConnection, id: Uuid) -> Result<Option<Question>, StoreError> {
    let at = Selection {
        question: Some(id),
        ..Selection::default()
    };
    Ok(question_rows(connection, at)
        .await?
        .0
        .pop()
        .map(|row| row.question))
}

/// A decision as its five columns; all empty for an open question.
#[derive(Default)]
struct DecisionColumns<'a> {
    option: Option<String>,
    text: Option<&'a str>,
    reason: Option<&'a str>,
    by: Option<Uuid>,
    at: Option<i64>,
}

fn decision_columns(decision: Option<&Decision>) -> DecisionColumns<'_> {
    let Some(decision) = decision else {
        return DecisionColumns::default();
    };
    let (option, text) = match &decision.choice {
        Some(Choice::Option(value)) => (Some(format!("O{value}")), None),
        Some(Choice::Text(text)) => (None, Some(text.as_str())),
        None => (None, None),
    };
    DecisionColumns {
        option,
        text,
        reason: decision.reason.as_deref(),
        by: Some(decision.by.as_uuid()),
        at: Some(decision.at.as_millis()),
    }
}

async fn insert_row(
    connection: &mut SqliteConnection,
    record: &QuestionRecord,
) -> Result<(), AppError> {
    let id = record.id.as_uuid();
    let project = record.project.as_uuid();
    let number = i64::from(record.key.number());
    let origin = record.origin.as_uuid();
    let created_by = record.created_by.as_uuid();
    let created_at = record.created_at.as_millis();
    let updated_at = record.updated_at.as_millis();
    let version = record.version.value();
    sqlx::query!(
        "INSERT INTO questions (id, project_id, number, title, description, state, occasion,
                                origin_actor_id, origin_session, created_by, created_at,
                                updated_at, version)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        id,
        project,
        number,
        record.title.as_str(),
        record.description,
        "open",
        record.occasion,
        origin,
        record.origin_session,
        created_by,
        created_at,
        updated_at,
        version,
    )
    .execute(connection)
    .await
    .map_err(database)?;
    Ok(())
}

async fn insert_lists(
    connection: &mut SqliteConnection,
    record: &QuestionRecord,
) -> Result<(), AppError> {
    let id = record.id.as_uuid();
    for (position, tag) in (0_i64..).zip(&record.tags) {
        sqlx::query!(
            "INSERT INTO question_tags (question_id, tag, position) VALUES (?, ?, ?)",
            id,
            tag,
            position,
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    }
    for ticket in &record.tickets {
        let project = ticket.project().as_str();
        let number = i64::from(ticket.number());
        sqlx::query!(
            "INSERT INTO question_tickets (question_id, ticket_id)
             SELECT ?, t.id FROM tickets t JOIN projects p ON p.id = t.project_id
             WHERE p.key = ? AND t.number = ?",
            id,
            project,
            number,
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    }
    for (position, target) in (0_i64..).zip(&record.depends_on) {
        let (question, ticket) = match target {
            QuestionRef::Question(key) => (question_id(&mut *connection, key).await?, None),
            QuestionRef::Ticket(key) => (
                None,
                crate::tickets::ticket_id(&mut *connection, key).await?,
            ),
        };
        sqlx::query!(
            "INSERT INTO question_dependencies (question_id, depends_on_question_id,
                                               depends_on_ticket_id, position)
             VALUES (?, ?, ?, ?)",
            id,
            question,
            ticket,
            position,
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    }
    Ok(())
}

#[async_trait]
impl QuestionWrite for Write {
    async fn take_question_number(&mut self, project: Id<Project>) -> Result<u32, AppError> {
        let uuid = project.as_uuid();
        let value = sqlx::query_scalar!(
            r#"UPDATE projects SET next_question_number = next_question_number + 1 WHERE id = ?
               RETURNING next_question_number - 1 AS "number!: i64""#,
            uuid,
        )
        .fetch_optional(&mut *self.0)
        .await
        .map_err(database)?
        .ok_or_else(|| nullradix_app::projects::not_found(project))?;
        Ok(number(value, "question counter")?)
    }

    async fn question(&mut self, key: &QuestionKey) -> Result<Option<Question>, AppError> {
        let Some(id) = question_id(&mut self.0, key).await? else {
            return Ok(None);
        };
        Ok(one(&mut self.0, id).await?)
    }

    async fn insert_question(&mut self, question: &Question) -> Result<Change, AppError> {
        let record = question.record();
        insert_row(&mut self.0, record).await?;
        insert_lists(&mut self.0, record).await?;
        Ok(change_feed::record(
            &mut self.0,
            Entity::Question,
            record.id.as_uuid(),
            ChangeOp::Insert,
            record.created_at,
        )
        .await?)
    }

    async fn update_question(
        &mut self,
        question: &Question,
        expected: Version,
    ) -> Result<Change, AppError> {
        let record = question.record();
        let id = record.id.as_uuid();
        let state = record.state.as_str();
        let columns = decision_columns(record.decision.as_ref());
        let updated_at = record.updated_at.as_millis();
        let version = record.version.value();
        let expected = expected.value();
        let updated = sqlx::query!(
            "UPDATE questions SET state = ?, decision_option = ?, decision_text = ?,
                                  decision_reason = ?, decided_by = ?, decided_at = ?,
                                  updated_at = ?, version = ?
             WHERE id = ? AND version = ?",
            state,
            columns.option,
            columns.text,
            columns.reason,
            columns.by,
            columns.at,
            updated_at,
            version,
            id,
            expected,
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        if updated.rows_affected() == 0 {
            let exists = sqlx::query_scalar!("SELECT 1 FROM questions WHERE id = ?", id)
                .fetch_optional(&mut *self.0)
                .await;
            return Err(missed_guard(
                exists,
                AppError::version_conflict("question", &record.key),
                not_found(&record.key),
            ));
        }
        Ok(change_feed::record(
            &mut self.0,
            Entity::Question,
            id,
            ChangeOp::Update,
            record.updated_at,
        )
        .await?)
    }

    async fn insert_research(&mut self, research: &Research) -> Result<(), AppError> {
        let id = research.id.as_uuid();
        let question = research.question.as_uuid();
        let actor = research.actor.as_uuid();
        let created_at = research.created_at.as_millis();
        sqlx::query!(
            "INSERT INTO question_research (id, question_id, actor_id, context, created_at)
             VALUES (?, ?, ?, ?, ?)",
            id,
            question,
            actor,
            research.context,
            created_at,
        )
        .execute(&mut *self.0)
        .await
        .map_err(database)?;
        for option in &research.options {
            let value = i64::from(option.number);
            let pros = serde_json::to_string(&option.pros).map_err(AppError::internal)?;
            let cons = serde_json::to_string(&option.cons).map_err(AppError::internal)?;
            let recommended = i64::from(option.recommended);
            sqlx::query!(
                "INSERT INTO question_options (question_id, research_id, number, title, text, pros,
                                               cons, recommended)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                question,
                id,
                value,
                option.title,
                option.text,
                pros,
                cons,
                recommended,
            )
            .execute(&mut *self.0)
            .await
            .map_err(database)?;
        }
        Ok(())
    }

    async fn research(&mut self, question: Id<Question>) -> Result<Vec<Research>, AppError> {
        let at = Selection {
            question: Some(question.as_uuid()),
            ..Selection::default()
        };
        Ok(research(&mut self.0, at)
            .await?
            .remove(&question.as_uuid())
            .unwrap_or_default())
    }

    async fn open_questions(&mut self, ticket: Id<Ticket>) -> Result<Vec<OpenQuestion>, AppError> {
        let uuid = ticket.as_uuid();
        Ok(open_by_ticket(&mut self.0, None, None, Some(uuid))
            .await?
            .remove(&uuid)
            .unwrap_or_default())
    }

    async fn questions_naming(&mut self, ticket: Id<Ticket>) -> Result<Vec<Question>, AppError> {
        let uuid = ticket.as_uuid();
        let ids = sqlx::query_scalar!(
            r#"SELECT q.id AS "id: Uuid" FROM questions q
               WHERE q.id IN (SELECT question_id FROM question_tickets WHERE ticket_id = ?1)
                  OR q.id IN (SELECT question_id FROM question_dependencies
                              WHERE depends_on_ticket_id = ?1)
               ORDER BY q.project_id, q.number"#,
            uuid,
        )
        .fetch_all(&mut *self.0)
        .await
        .map_err(database)?;
        let mut questions = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(question) = one(&mut self.0, id).await? {
                questions.push(question);
            }
        }
        Ok(questions)
    }

    async fn note_tickets(&mut self, history: &[HistoryEntry]) -> Result<(), AppError> {
        insert_history(&mut self.0, history).await
    }
}

#[async_trait]
impl QuestionRepository for Store {
    async fn list(&self, project: Id<Project>) -> Result<Option<Vec<QuestionRow>>, AppError> {
        let uuid = project.as_uuid();
        // one read transaction keeps questions, lists and states on the same snapshot
        let mut read = self.reader.begin().await.map_err(database)?;
        let found = sqlx::query_scalar!("SELECT 1 FROM projects WHERE id = ?", uuid)
            .fetch_optional(&mut *read)
            .await
            .map_err(database)?;
        if found.is_none() {
            return Ok(None);
        }
        let at = Selection {
            project: Some(uuid),
            ..Selection::default()
        };
        let (rows, _) = question_rows(&mut read, at).await?;
        read.commit().await.map_err(database)?;
        Ok(Some(rows))
    }

    async fn get(&self, key: &QuestionKey) -> Result<Option<QuestionRead>, AppError> {
        let mut read = self.reader.begin().await.map_err(database)?;
        let Some(id) = question_id(&mut read, key).await? else {
            return Ok(None);
        };
        let at = Selection {
            question: Some(id),
            ..Selection::default()
        };
        let (mut rows, mut rounds) = question_rows(&mut read, at).await?;
        let Some(row) = rows.pop() else {
            return Ok(None);
        };
        let comments: Vec<QuestionComment> = comments::question_comments(&mut read, id).await?;
        read.commit().await.map_err(database)?;
        Ok(Some(QuestionRead {
            row,
            research: rounds.remove(&id).unwrap_or_default(),
            comments,
        }))
    }

    async fn exists(&self, key: &QuestionKey) -> Result<bool, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        Ok(question_id(&mut read, key).await?.is_some())
    }

    async fn researched_by(
        &self,
        key: &QuestionKey,
        agent: &str,
        since: Timestamp,
    ) -> Result<bool, AppError> {
        let mut read = self.reader.acquire().await.map_err(database)?;
        let Some(question) = question_id(&mut read, key).await? else {
            return Ok(false);
        };
        let since = since.as_millis();
        let found = sqlx::query_scalar!(
            r#"SELECT r.id AS "id: Uuid" FROM question_research r JOIN actors a ON a.id = r.actor_id
               WHERE r.question_id = ? AND a.name = ? AND r.created_at >= ?
               LIMIT 1"#,
            question,
            agent,
            since,
        )
        .fetch_optional(&mut *read)
        .await
        .map_err(database)?;
        Ok(found.is_some())
    }
}
