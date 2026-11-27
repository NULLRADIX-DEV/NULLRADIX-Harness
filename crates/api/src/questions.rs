use axum::Json;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use nullradix_app::AppError;
use nullradix_app::auth::{Principal, decision_needs_person};
use nullradix_app::comments::Author;
use nullradix_app::questions::{
    Answer, DependencyState, NewQuestion, NewResearch, OpenQuestion, QuestionComment,
    QuestionDetail, QuestionFilter, QuestionView,
};
use nullradix_domain::{Choice, Id, OptionDraft, QuestionOption, QuestionState, Research};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::auth::{AgentContext, refuse_agent};
use crate::comments::ActorKindDto;
use crate::tickets::TicketStatusDto;
use crate::{API_PREFIX, ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum QuestionStateDto {
    Open,
    Answered,
    Discarded,
}

impl From<QuestionState> for QuestionStateDto {
    fn from(state: QuestionState) -> Self {
        match state {
            QuestionState::Open => Self::Open,
            QuestionState::Answered => Self::Answered,
            QuestionState::Discarded => Self::Discarded,
        }
    }
}

/// An open question that holds a ticket back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OpenQuestionDto {
    /// Human key such as `ARC-Q0007`.
    pub key: String,
    pub title: String,
}

impl From<OpenQuestion> for OpenQuestionDto {
    fn from(question: OpenQuestion) -> Self {
        Self {
            key: question.key.to_string(),
            title: question.title,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QuestionTicketDto {
    pub key: String,
    pub title: String,
    pub status: TicketStatusDto,
}

/// A dependency that is still open, with its question state or ticket status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WaitingDto {
    pub key: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DecisionDto {
    /// Chosen option such as `O2`; none for an own answer or a discard.
    pub option: Option<String>,
    /// Own answer instead of an option.
    pub text: Option<String>,
    pub reason: Option<String>,
    pub by_name: String,
    pub at: i64,
    /// Taken over by the dispatcher's autopilot.
    pub autopilot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestionDto {
    /// Human key such as `ARC-Q0007`.
    pub key: String,
    pub id: Uuid,
    pub project_id: Uuid,
    pub number: u32,
    pub title: String,
    pub description: String,
    pub state: QuestionStateDto,
    pub occasion: Option<String>,
    pub tags: Vec<String>,
    /// Tickets the question is about; an open question holds them back.
    pub tickets: Vec<QuestionTicketDto>,
    /// Questions or tickets the answer waits for, in the order given.
    pub depends_on: Vec<String>,
    pub waiting_on: Vec<WaitingDto>,
    /// Open and waiting for nothing: a person can decide it now.
    pub ready: bool,
    pub origin_name: String,
    pub origin_kind: ActorKindDto,
    pub origin_session: Option<String>,
    pub decision: Option<DecisionDto>,
    pub researched: bool,
    /// Options of the newest round.
    pub option_count: u32,
    pub created_by_id: Uuid,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: i64,
}

impl From<QuestionView> for QuestionDto {
    fn from(view: QuestionView) -> Self {
        let row = view.row;
        let record = row.question.record();
        let decision = record.decision.as_ref().map(|decision| DecisionDto {
            option: match &decision.choice {
                Some(Choice::Option(number)) => Some(format!("O{number}")),
                _ => None,
            },
            text: match &decision.choice {
                Some(Choice::Text(text)) => Some(text.clone()),
                _ => None,
            },
            reason: decision.reason.clone(),
            by_name: row.decided_by_name.clone().unwrap_or_default(),
            at: decision.at.as_millis(),
            autopilot: decision.by_autopilot(),
        });
        let waiting = |(target, state): &(_, DependencyState)| WaitingDto {
            key: ToString::to_string(target),
            state: state.as_str().to_owned(),
        };
        Self {
            key: record.key.to_string(),
            id: record.id.as_uuid(),
            project_id: record.project.as_uuid(),
            number: record.key.number(),
            title: record.title.as_str().to_owned(),
            description: record.description.clone(),
            state: record.state.into(),
            occasion: record.occasion.clone(),
            tags: record.tags.clone(),
            tickets: row
                .tickets
                .iter()
                .map(|ticket| QuestionTicketDto {
                    key: ticket.key.to_string(),
                    title: ticket.title.clone(),
                    status: ticket.status.into(),
                })
                .collect(),
            depends_on: record.depends_on.iter().map(ToString::to_string).collect(),
            waiting_on: view.waiting_on.iter().map(waiting).collect(),
            ready: view.ready,
            origin_name: row.origin_name.clone(),
            origin_kind: row.origin_kind.into(),
            origin_session: record.origin_session.clone(),
            decision,
            researched: row.research_rounds > 0,
            option_count: row.option_count,
            created_by_id: record.created_by.as_uuid(),
            created_at: record.created_at.as_millis(),
            updated_at: record.updated_at.as_millis(),
            version: record.version.value(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OptionDto {
    /// Key to answer by, such as `O2`.
    pub key: String,
    pub title: String,
    pub text: String,
    pub pros: Vec<String>,
    pub cons: Vec<String>,
    pub recommended: bool,
}

impl From<&QuestionOption> for OptionDto {
    fn from(option: &QuestionOption) -> Self {
        Self {
            key: option.key(),
            title: option.title.clone(),
            text: option.text.clone(),
            pros: option.pros.clone(),
            cons: option.cons.clone(),
            recommended: option.recommended,
        }
    }
}

/// One round of options; only the newest one may be chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResearchDto {
    pub id: Uuid,
    pub context: String,
    pub options: Vec<OptionDto>,
    pub created_at: i64,
}

impl From<&Research> for ResearchDto {
    fn from(research: &Research) -> Self {
        Self {
            id: research.id.as_uuid(),
            context: research.context.clone(),
            options: research.options.iter().map(OptionDto::from).collect(),
            created_at: research.created_at.as_millis(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestionCommentDto {
    pub id: Uuid,
    pub question_key: String,
    pub author_id: Uuid,
    pub author_name: String,
    pub author_kind: ActorKindDto,
    pub body: String,
    pub created_at: i64,
}

impl From<QuestionComment> for QuestionCommentDto {
    fn from(line: QuestionComment) -> Self {
        let record = line.comment.record();
        Self {
            id: record.id.as_uuid(),
            question_key: line.question_key.to_string(),
            author_id: record.author.as_uuid(),
            author_name: line.author_name,
            author_kind: line.author_kind.into(),
            body: record.body.as_str().to_owned(),
            created_at: record.created_at.as_millis(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestionDetailDto {
    pub question: QuestionDto,
    /// Rounds of options, oldest first.
    pub research: Vec<ResearchDto>,
    /// Oldest first.
    pub comments: Vec<QuestionCommentDto>,
}

impl From<QuestionDetail> for QuestionDetailDto {
    fn from(detail: QuestionDetail) -> Self {
        Self {
            research: detail.research.iter().map(ResearchDto::from).collect(),
            comments: detail
                .comments
                .into_iter()
                .map(QuestionCommentDto::from)
                .collect(),
            question: detail.view.into(),
        }
    }
}

/// Questions of the project in every state, before the filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QuestionCounts {
    pub open: u32,
    pub answered: u32,
    pub discarded: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QuestionList {
    /// Ready questions first, oldest first within.
    pub items: Vec<QuestionDto>,
    pub counts: QuestionCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct QuestionCommentList {
    /// Oldest first.
    pub items: Vec<QuestionCommentDto>,
}

/// A proposed answer; the question numbers it O1, O2 ...
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewOption {
    pub title: String,
    pub text: String,
    #[serde(default)]
    pub pros: Vec<String>,
    #[serde(default)]
    pub cons: Vec<String>,
    /// At most one option may be recommended.
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateQuestion {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Why it came up, such as `planning ARC-0007`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occasion: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Keys of tickets of the project it is about; they wait until it is decided.
    #[serde(default)]
    pub tickets: Vec<String>,
    /// Questions or tickets the answer waits for.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Up to five options; the first round of the question.
    #[serde(default)]
    pub options: Vec<NewOption>,
    /// Name to ask under, such as `claude`; the acting person when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Agent session of the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// An answer: exactly one of `option` and `text`; only a person answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnswerQuestion {
    /// An option of the newest round, such as `O2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
    /// An own answer instead of an option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Version the answer is based on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    /// Name of the person who decides; the acting person when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Agent session of the caller; agents never decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// Discarding needs a reason; reopening takes none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecideQuestion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// A round of context and options for a question; options count on after the earlier rounds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitResearch {
    /// Markdown with what the code does today and the recommendation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Up to five options; at most one recommended.
    #[serde(default)]
    pub options: Vec<NewOption>,
    /// Name to write under, such as `research-ARC-Q0007`; the acting person when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Agent session of the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// A new comment on a question; leading and trailing whitespace is dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddQuestionComment {
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct ProjectQuestionsPath {
    /// Project id.
    project_id: Uuid,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct QuestionPath {
    /// Question key such as `ARC-Q0007`; case and padding do not matter.
    question_key: String,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct QuestionQuery {
    /// open, answered, discarded or all; all when left out.
    state: Option<String>,
    /// Only questions about this ticket.
    ticket: Option<String>,
    /// Only questions with this tag.
    tag: Option<String>,
    /// Case-insensitive text in key, title, description, occasion, tags or tickets.
    q: Option<String>,
}

fn author<'a>(
    principal: &Principal,
    name: Option<&'a str>,
    session: Option<&'a str>,
) -> Author<'a> {
    Author {
        by_person: matches!(principal, Principal::Session { .. }),
        name,
        session,
    }
}

fn version(value: Option<i64>) -> Result<Option<nullradix_domain::Version>, AppError> {
    value.map(AppError::client_version).transpose()
}

#[utoipa::path(
    get,
    path = "/projects/{projectId}/questions",
    tag = "questions",
    params(ProjectQuestionsPath, QuestionQuery),
    responses(
        (status = 200, body = QuestionList),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn list_questions(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ProjectQuestionsPath>,
    ApiQuery(query): ApiQuery<QuestionQuery>,
) -> Result<Json<QuestionList>, ApiError> {
    let filter = QuestionFilter::parse(
        query.state.as_deref(),
        query.ticket.as_deref(),
        query.tag.as_deref(),
        query.q.as_deref(),
    )?;
    let list = state
        .questions
        .list(Id::from_uuid(path.project_id), &filter)
        .await?;
    Ok(Json(QuestionList {
        items: list.questions.into_iter().map(QuestionDto::from).collect(),
        counts: QuestionCounts {
            open: list.counts.open,
            answered: list.counts.answered,
            discarded: list.counts.discarded,
        },
    }))
}

#[utoipa::path(
    post,
    path = "/projects/{projectId}/questions",
    tag = "questions",
    params(ProjectQuestionsPath),
    request_body = CreateQuestion,
    responses(
        (status = 201, body = QuestionDetailDto,
            headers(("Location" = String, description = "URL of the new question"))),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn create_question(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<ProjectQuestionsPath>,
    ApiJson(input): ApiJson<CreateQuestion>,
) -> Result<(StatusCode, HeaderMap, Json<QuestionDetailDto>), ApiError> {
    let question = NewQuestion {
        title: input.title,
        description: input.description,
        occasion: input.occasion,
        tags: input.tags,
        tickets: input.tickets,
        depends_on: input.depends_on,
        options: input
            .options
            .into_iter()
            .map(|option| OptionDraft {
                title: option.title,
                text: option.text,
                pros: option.pros,
                cons: option.cons,
                recommended: option.recommended,
            })
            .collect(),
    };
    let author = author(
        &principal,
        input.author.as_deref(),
        input.session.as_deref(),
    );
    let created = QuestionDetailDto::from(
        state
            .questions
            .create(
                Id::from_uuid(path.project_id),
                &question,
                author,
                &state.person,
            )
            .await?,
    );
    let mut headers = HeaderMap::new();
    let location = format!("{API_PREFIX}/questions/{}", created.question.key);
    if let Ok(value) = HeaderValue::from_str(&location) {
        headers.insert(header::LOCATION, value);
    }
    Ok((StatusCode::CREATED, headers, Json(created)))
}

#[utoipa::path(
    get,
    path = "/questions/{questionKey}",
    tag = "questions",
    params(QuestionPath),
    responses(
        (status = 200, body = QuestionDetailDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn get_question(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<QuestionPath>,
) -> Result<Json<QuestionDetailDto>, ApiError> {
    Ok(Json(state.questions.get(&path.question_key).await?.into()))
}

#[utoipa::path(
    post,
    path = "/questions/{questionKey}/research",
    tag = "questions",
    params(QuestionPath),
    request_body = SubmitResearch,
    responses(
        (status = 201, body = QuestionDetailDto,
            headers(("Location" = String, description = "URL of the question"))),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, body = Problem),
        (status = 422, description = "Neither context nor options, or too many, too long or several recommended", body = Problem),
    )
)]
pub(crate) async fn submit_research(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<QuestionPath>,
    ApiJson(input): ApiJson<SubmitResearch>,
) -> Result<(StatusCode, HeaderMap, Json<QuestionDetailDto>), ApiError> {
    let research = NewResearch {
        context: input.context,
        options: input
            .options
            .into_iter()
            .map(|option| OptionDraft {
                title: option.title,
                text: option.text,
                pros: option.pros,
                cons: option.cons,
                recommended: option.recommended,
            })
            .collect(),
    };
    let author = author(
        &principal,
        input.author.as_deref(),
        input.session.as_deref(),
    );
    let detail = QuestionDetailDto::from(
        state
            .questions
            .add_research(&path.question_key, &research, author, &state.person)
            .await?,
    );
    let mut headers = HeaderMap::new();
    let location = format!("{API_PREFIX}/questions/{}", detail.question.key);
    if let Ok(value) = HeaderValue::from_str(&location) {
        headers.insert(header::LOCATION, value);
    }
    Ok((StatusCode::CREATED, headers, Json(detail)))
}

#[utoipa::path(
    post,
    path = "/questions/{questionKey}/answer",
    tag = "questions",
    params(QuestionPath),
    request_body = AnswerQuestion,
    responses(
        (status = 200, body = QuestionDetailDto),
        (status = 401, body = Problem),
        (status = 403, description = "`decision-needs-person`: the request says it comes from a run or session of an agent", body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "The question changed since it was read", body = Problem),
        (status = 422, description = "Not open, no person, or no single choice", body = Problem),
    )
)]
pub(crate) async fn answer_question(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Extension(context): Extension<AgentContext>,
    ApiPath(path): ApiPath<QuestionPath>,
    ApiJson(input): ApiJson<AnswerQuestion>,
) -> Result<Json<QuestionDetailDto>, ApiError> {
    refuse_agent(context, || decision_needs_person("answering a question"))?;
    let answer = Answer {
        option: input.option,
        text: input.text,
        reason: input.reason,
    };
    let author = author(
        &principal,
        input.author.as_deref(),
        input.session.as_deref(),
    );
    let detail = state
        .questions
        .answer(
            &path.question_key,
            &answer,
            author,
            &state.person,
            version(input.version)?,
        )
        .await?;
    Ok(Json(detail.into()))
}

#[utoipa::path(
    post,
    path = "/questions/{questionKey}/discard",
    tag = "questions",
    params(QuestionPath),
    request_body = DecideQuestion,
    responses(
        (status = 200, body = QuestionDetailDto),
        (status = 401, body = Problem),
        (status = 403, description = "`decision-needs-person`: the request says it comes from a run or session of an agent", body = Problem),
        (status = 404, body = Problem),
        (status = 409, body = Problem),
        (status = 422, description = "Not open, no person, or no reason", body = Problem),
    )
)]
pub(crate) async fn discard_question(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Extension(context): Extension<AgentContext>,
    ApiPath(path): ApiPath<QuestionPath>,
    ApiJson(input): ApiJson<DecideQuestion>,
) -> Result<Json<QuestionDetailDto>, ApiError> {
    refuse_agent(context, || decision_needs_person("discarding a question"))?;
    let author = author(
        &principal,
        input.author.as_deref(),
        input.session.as_deref(),
    );
    let detail = state
        .questions
        .discard(
            &path.question_key,
            input.reason.as_deref(),
            author,
            &state.person,
            version(input.version)?,
        )
        .await?;
    Ok(Json(detail.into()))
}

#[utoipa::path(
    post,
    path = "/questions/{questionKey}/reopen",
    tag = "questions",
    params(QuestionPath),
    request_body = DecideQuestion,
    responses(
        (status = 200, body = QuestionDetailDto),
        (status = 401, body = Problem),
        (status = 403, description = "`decision-needs-person`: the request says it comes from a run or session of an agent", body = Problem),
        (status = 404, body = Problem),
        (status = 409, body = Problem),
        (status = 422, description = "Open already, or no person", body = Problem),
    )
)]
pub(crate) async fn reopen_question(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Extension(context): Extension<AgentContext>,
    ApiPath(path): ApiPath<QuestionPath>,
    ApiJson(input): ApiJson<DecideQuestion>,
) -> Result<Json<QuestionDetailDto>, ApiError> {
    refuse_agent(context, || decision_needs_person("reopening a question"))?;
    let author = author(
        &principal,
        input.author.as_deref(),
        input.session.as_deref(),
    );
    let detail = state
        .questions
        .reopen(
            &path.question_key,
            input.reason.as_deref(),
            author,
            &state.person,
            version(input.version)?,
        )
        .await?;
    Ok(Json(detail.into()))
}

#[utoipa::path(
    get,
    path = "/questions/{questionKey}/comments",
    tag = "questions",
    params(QuestionPath),
    responses(
        (status = 200, body = QuestionCommentList),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn list_question_comments(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<QuestionPath>,
) -> Result<Json<QuestionCommentList>, ApiError> {
    let detail = state.questions.get(&path.question_key).await?;
    let items = detail
        .comments
        .into_iter()
        .map(QuestionCommentDto::from)
        .collect();
    Ok(Json(QuestionCommentList { items }))
}

#[utoipa::path(
    post,
    path = "/questions/{questionKey}/comments",
    tag = "questions",
    params(QuestionPath),
    request_body = AddQuestionComment,
    responses(
        (status = 201, body = QuestionCommentDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, description = "Empty body or an invalid author name", body = Problem),
    )
)]
pub(crate) async fn add_question_comment(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    ApiPath(path): ApiPath<QuestionPath>,
    ApiJson(input): ApiJson<AddQuestionComment>,
) -> Result<(StatusCode, Json<QuestionCommentDto>), ApiError> {
    let author = author(
        &principal,
        input.author.as_deref(),
        input.session.as_deref(),
    );
    let line = state
        .questions
        .comment(&path.question_key, &input.body, author, &state.person)
        .await?;
    Ok((StatusCode::CREATED, Json(line.into())))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};

    use crate::tests::{
        TestApi, as_person, bearer, in_run, in_session, open_session, send, test_api,
    };

    async fn call(api: &TestApi, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let request = bearer(Request::builder().method(method).uri(path), &api.token)
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    async fn with_ticket() -> (TestApi, String) {
        let api = test_api().await;
        let project = api.projects.create("ARC", "Arches").await.unwrap();
        let board = api
            .boards
            .create(project.id(), "Main", &api.person)
            .await
            .unwrap()
            .board;
        let path = format!(
            "/api/v1/projects/{}/boards/{}/tickets",
            project.id().as_uuid(),
            board.id().as_uuid()
        );
        let ticket = json!({ "title": "Login", "acceptance": "works" });
        assert_eq!(call(&api, "POST", &path, Some(ticket)).await.0, 201);
        let questions = format!("/api/v1/projects/{}/questions", project.id().as_uuid());
        (api, questions)
    }

    #[tokio::test]
    async fn a_question_is_asked_answered_and_lets_its_ticket_go() {
        let (api, questions) = with_ticket().await;
        let input = json!({
            "title": "Which store?",
            "tickets": ["arc-1"],
            "options": [
                { "title": "File", "text": "A file", "pros": ["simple"] },
                { "title": "Db", "text": "SQLite", "recommended": true },
            ],
            "author": "claude",
            "session": "s-1",
        });
        let (status, created) = call(&api, "POST", &questions, Some(input)).await;
        assert_eq!(status, 201);
        assert_eq!(created["question"]["key"], "ARC-Q0001");
        assert_eq!(created["question"]["originKind"], "session");
        assert_eq!(created["research"][0]["options"][1]["key"], "O2");
        assert_eq!(created["research"][0]["options"][1]["recommended"], true);

        let (_, ticket) = call(&api, "GET", "/api/v1/tickets/ARC-1", None).await;
        assert_eq!(ticket["ticket"]["openQuestions"][0]["key"], "ARC-Q0001");
        let start = json!({ "status": "in-progress" });
        let (status, problem) = call(
            &api,
            "POST",
            "/api/v1/tickets/ARC-1/move",
            Some(start.clone()),
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("question-open"))
        );

        let agent = json!({ "option": "O2", "session": "s-1" });
        let (status, problem) =
            call(&api, "POST", "/api/v1/questions/arc-q1/answer", Some(agent)).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("human-only"))
        );
        let stale = json!({ "option": "O2", "version": 9 });
        let (status, problem) =
            call(&api, "POST", "/api/v1/questions/arc-q1/answer", Some(stale)).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (409, Some("version-conflict"))
        );
        let answer = json!({ "option": "O2", "reason": "fits", "version": 1 });
        let (status, answered) = call(
            &api,
            "POST",
            "/api/v1/questions/arc-q1/answer",
            Some(answer),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(answered["question"]["state"], "answered");
        assert_eq!(answered["question"]["decision"]["option"], "O2");
        assert_eq!(answered["question"]["decision"]["byName"], "tester");
        let (status, _) = call(&api, "POST", "/api/v1/tickets/ARC-1/move", Some(start)).await;
        assert_eq!(status, 200);

        let (status, list) = call(&api, "GET", &format!("{questions}?state=answered"), None).await;
        assert_eq!(status, 200);
        assert_eq!(list["items"][0]["key"], "ARC-Q0001");
        assert_eq!(
            list["counts"],
            json!({ "open": 0, "answered": 1, "discarded": 0 })
        );
    }

    #[tokio::test]
    async fn deciding_stays_with_the_person_and_not_with_a_run_or_session_of_an_agent() {
        let (api, questions) = with_ticket().await;
        let asked = json!({
            "title": "Which store?",
            "options": [{ "title": "File", "text": "A file" }, { "title": "Db", "text": "SQLite" }],
        });
        assert_eq!(call(&api, "POST", &questions, Some(asked)).await.0, 201);
        let cookie = open_session(&api).await;
        let state = || async { call(&api, "GET", "/api/v1/questions/ARC-Q1", None).await.1 };

        // each decision refuses a token and a session that name an agent, and changes nothing
        let decisions = [
            (
                "answer",
                json!({ "option": "O1", "reason": "fits" }),
                "open",
            ),
            ("discard", json!({ "reason": "not needed" }), "open"),
        ];
        for (action, body, expected) in &decisions {
            let path = format!("/api/v1/questions/ARC-Q1/{action}");
            for mark in [in_run, in_session] {
                for as_session in [false, true] {
                    let builder = if as_session {
                        as_person(Request::post(path.as_str()), &cookie)
                    } else {
                        bearer(Request::post(path.as_str()), &api.token)
                    };
                    let request = mark(builder)
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap();
                    let (status, _, problem) = send(&api.router, request).await;
                    assert_eq!(status, StatusCode::FORBIDDEN, "{action} {problem}");
                    assert_eq!(problem["code"], "decision-needs-person", "{action}");
                }
            }
            assert_eq!(state().await["question"]["state"], *expected, "{action}");
        }

        // the person in the app decides; then reopening is refused the same way
        let post_as_person = |action: &'static str, body: Value| {
            let request = as_person(
                Request::post(format!("/api/v1/questions/ARC-Q1/{action}")),
                &cookie,
            )
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
            send(&api.router, request)
        };
        let (status, _, answered) = post_as_person("answer", decisions[0].1.clone()).await;
        assert_eq!(status, StatusCode::OK, "{answered}");
        assert_eq!(answered["question"]["state"], "answered");
        for mark in [in_run, in_session] {
            let request = mark(bearer(
                Request::post("/api/v1/questions/ARC-Q1/reopen"),
                &api.token,
            ))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "reason": "again" }).to_string()))
            .unwrap();
            let (status, _, problem) = send(&api.router, request).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
            assert_eq!(problem["code"], "decision-needs-person");
        }
        assert_eq!(state().await["question"]["state"], "answered");
        let (status, _, reopened) = post_as_person("reopen", json!({ "reason": "again" })).await;
        assert_eq!(status, StatusCode::OK, "{reopened}");
        assert_eq!(reopened["question"]["state"], "open");
        let (status, _, discarded) =
            post_as_person("discard", json!({ "reason": "not needed" })).await;
        assert_eq!(status, StatusCode::OK, "{discarded}");
        assert_eq!(discarded["question"]["state"], "discarded");
    }

    #[tokio::test]
    async fn questions_refuse_bad_input_with_legacy_codes() {
        let (api, questions) = with_ticket().await;
        let cases = [
            (
                "GET",
                format!("{questions}?state=offen"),
                None,
                422,
                "question-state-invalid",
            ),
            (
                "GET",
                String::from("/api/v1/questions/ARC-7"),
                None,
                422,
                "question-id-invalid",
            ),
            (
                "GET",
                String::from("/api/v1/questions/ARC-Q7"),
                None,
                404,
                "question-not-found",
            ),
            (
                "POST",
                questions.clone(),
                Some(json!({ "title": "x", "tickets": ["ARC-9"] })),
                422,
                "question-ticket-unknown",
            ),
            (
                "GET",
                format!("/api/v1/projects/{}/questions", uuid::Uuid::nil()),
                None,
                404,
                "project-not-found",
            ),
        ];
        for (method, path, body, status, code) in cases {
            let (found, problem) = call(&api, method, &path, body).await;
            assert_eq!(
                (found, problem["code"].as_str()),
                (status, Some(code)),
                "{path}"
            );
        }
        let (status, _) = call(&api, "POST", &questions, Some(json!({ "title": "Why?" }))).await;
        assert_eq!(status, 201);
        let (status, problem) = call(
            &api,
            "POST",
            "/api/v1/questions/ARC-Q1/discard",
            Some(json!({})),
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("decision-reason-missing"))
        );
        let (status, comment) = call(
            &api,
            "POST",
            "/api/v1/questions/ARC-Q1/comments",
            Some(json!({ "body": " Found it " })),
        )
        .await;
        assert_eq!((status, comment["body"].as_str()), (201, Some("Found it")));
        let (_, list) = call(&api, "GET", "/api/v1/questions/ARC-Q1/comments", None).await;
        assert_eq!(list["items"][0]["questionKey"], "ARC-Q0001");
        let (status, problem) = call(
            &api,
            "POST",
            "/api/v1/questions/ARC-Q1/reopen",
            Some(json!({})),
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("question-state-invalid"))
        );
    }

    #[tokio::test]
    async fn a_research_adds_a_round_whose_options_count_on() {
        let (api, questions) = with_ticket().await;
        let asked =
            json!({ "title": "Which store?", "options": [{ "title": "File", "text": "A file" }] });
        assert_eq!(call(&api, "POST", &questions, Some(asked)).await.0, 201);
        let research = json!({
            "context": " Today it is a file. ",
            "options": [
                { "title": "Db", "text": "SQLite", "pros": ["safe"], "recommended": true },
                { "title": "Cloud", "text": "Hosted" },
            ],
            "author": "research-ARC-Q0001",
            "session": "s-9",
        });
        let (status, detail) = call(
            &api,
            "POST",
            "/api/v1/questions/arc-q1/research",
            Some(research),
        )
        .await;
        assert_eq!(status, 201);
        assert_eq!(detail["research"].as_array().unwrap().len(), 2);
        assert_eq!(detail["research"][1]["context"], "Today it is a file.");
        assert_eq!(detail["research"][1]["options"][0]["key"], "O2");
        assert_eq!(detail["research"][1]["options"][0]["recommended"], true);
        assert_eq!(detail["research"][1]["options"][1]["key"], "O3");
        assert_eq!(detail["question"]["researched"], true);
        // a context alone is a research too
        let (status, detail) = call(
            &api,
            "POST",
            "/api/v1/questions/ARC-Q1/research",
            Some(json!({ "context": "More." })),
        )
        .await;
        assert_eq!(status, 201);
        assert_eq!(
            detail["research"][2]["options"].as_array().unwrap().len(),
            0
        );
        // the refusals
        let refuse = |body: Value| {
            call(
                &api,
                "POST",
                "/api/v1/questions/ARC-Q1/research",
                Some(body),
            )
        };
        let (status, problem) = refuse(json!({})).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("research-invalid"))
        );
        let two = json!([
            { "title": "A", "text": "x", "recommended": true },
            { "title": "B", "text": "y", "recommended": true },
        ]);
        let (_, problem) = refuse(json!({ "options": two })).await;
        assert_eq!(problem["code"], "research-recommended-multiple");
        let six: Vec<Value> = (0..6)
            .map(|n| json!({ "title": n.to_string(), "text": "t" }))
            .collect();
        assert_eq!(
            refuse(json!({ "options": six })).await.1["code"],
            "research-invalid"
        );
        let blank = json!([{ "title": "A", "text": " " }]);
        assert_eq!(
            refuse(json!({ "options": blank })).await.1["code"],
            "research-invalid"
        );
        let long_text = json!([{ "title": "A", "text": "x".repeat(4001) }]);
        assert_eq!(
            refuse(json!({ "options": long_text })).await.1["code"],
            "value-too-long"
        );
        let exact = json!([{ "title": "A", "text": "x".repeat(4000) }]);
        assert_eq!(refuse(json!({ "options": exact })).await.0, 201);
        let long_context = json!({ "context": "c".repeat(20_001) });
        assert_eq!(refuse(long_context).await.1["code"], "value-too-long");
        let (status, problem) = call(
            &api,
            "POST",
            "/api/v1/questions/ARC-Q9/research",
            Some(json!({ "context": "x" })),
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (404, Some("question-not-found"))
        );
    }

    #[tokio::test]
    async fn a_research_run_needs_a_known_question_with_a_board_that_has_a_repository() {
        let (api, questions) = with_ticket().await;
        let (status, problem) = call(
            &api,
            "POST",
            "/api/v1/questions/ARC-Q9/research-runs",
            Some(json!({})),
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (404, Some("question-not-found"))
        );
        let (status, problem) = call(
            &api,
            "POST",
            "/api/v1/questions/arc-0001/research-runs",
            Some(json!({})),
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("question-id-invalid"))
        );
        assert_eq!(
            call(&api, "POST", &questions, Some(json!({ "title": "Which?" })))
                .await
                .0,
            201
        );
        let (status, problem) = call(
            &api,
            "POST",
            "/api/v1/questions/ARC-Q1/research-runs",
            Some(json!({})),
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("board-repo-missing"))
        );
    }
}
