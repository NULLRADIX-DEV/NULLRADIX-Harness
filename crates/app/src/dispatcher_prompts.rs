//! The prompt templates of the dispatcher: shipped defaults, own overrides and previews.

use std::sync::Arc;

use nullradix_domain::{
    AnsweredQuestion, DispatcherConfig, MergeRun, PlaceholderInfo, PlanRun, PromptKind, PromptRun,
    QuestionState, ResearchRun, TicketKey, Version, WorkRun, default_template, describe,
    missing_parts, pending_feedback, placeholders, render_prompt, required_parts, size_problem,
    ticket_branch, unknown_placeholders,
};

use crate::dispatcher_config::{ConfigView, DispatcherConfigService};
use crate::questions::{QuestionFilter, QuestionService};
use crate::tickets::{TicketDetail, TicketService};
use crate::{AppError, FieldProblem};

/// Target branch of a merge when the ticket names none.
pub const DEFAULT_TARGET: &str = "main";

const SAMPLE_TICKET: &str = "ARC-0042";
const SAMPLE_QUESTION: &str = "ARC-Q0007";
const SAMPLE_TITLE: &str = "Sample ticket: preview of the instruction";

const ATTEMPTS_STAND_IN: &str = "(preview) The report of earlier failed merge sessions appears here from the second session on.";

/// The feedback a person returned the ticket with since the agent started last.
fn feedback_of(detail: &TicketDetail) -> Vec<String> {
    pending_feedback(detail.comments.iter().map(|line| {
        let record = line.comment.record();
        (record.system_marker, record.body.as_str())
    }))
}

/// A prompt kind with its default, its own template and what an editor needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptView {
    pub kind: PromptKind,
    pub label: &'static str,
    pub default: &'static str,
    /// The own template; none means the default applies.
    pub template: Option<String>,
    pub placeholders: Vec<PlaceholderInfo>,
    /// Labels of the parts every template of this kind must keep.
    pub required: Vec<&'static str>,
}

impl PromptView {
    fn of(config: &DispatcherConfig, kind: PromptKind) -> Self {
        Self {
            kind,
            label: kind.label(),
            default: default_template(kind),
            template: config.prompts.get(kind).map(str::to_owned),
            placeholders: placeholders(kind).to_vec(),
            required: required_parts(kind).iter().map(|part| part.label).collect(),
        }
    }
}

/// The prompt kinds in order, with the version of the configuration they live in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptList {
    pub items: Vec<PromptView>,
    pub version: Version,
}

/// One prompt kind with the version of the configuration it lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptState {
    pub prompt: PromptView,
    pub version: Version,
}

/// What to render: a kind, optionally for a ticket or question, optionally with a draft template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRequest {
    pub kind: PromptKind,
    /// Ticket key for work, plan and merge, question key for research; none uses sample values.
    pub key: Option<String>,
    /// Rendered instead of the stored template, without saving it.
    pub template: Option<String>,
}

/// An instruction as the agent would get it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPreview {
    pub kind: PromptKind,
    pub key: Option<String>,
    pub sample: bool,
    pub prompt: String,
    /// Placeholders that show a stand-in text because their data does not exist yet.
    pub stand_ins: Vec<&'static str>,
}

fn invalid_template(fields: Vec<FieldProblem>) -> AppError {
    AppError::invalid(
        "dispatcher-prompt-invalid",
        "the prompt template is invalid",
    )
    .with_fields(fields)
}

/// The problems of a template as field problems; none when it may be used.
fn template_problems(kind: PromptKind, template: &str) -> Vec<FieldProblem> {
    if let Some(message) = size_problem(template) {
        return vec![FieldProblem {
            field: String::from("template"),
            message,
        }];
    }
    let mut fields = Vec::new();
    let unknown = unknown_placeholders(kind, template);
    if !unknown.is_empty() {
        let names: Vec<String> = unknown
            .iter()
            .map(|name| format!("{{{{{name}}}}}"))
            .collect();
        fields.push(FieldProblem {
            field: String::from("template"),
            message: format!("uses unknown placeholders {}", names.join(", ")),
        });
    }
    let missing: Vec<&str> = missing_parts(kind, template)
        .iter()
        .map(|part| part.label)
        .collect();
    if !missing.is_empty() {
        fields.push(FieldProblem {
            field: String::from("template"),
            message: format!("lacks required parts: {}", missing.join("; ")),
        });
    }
    fields
}

/// A template as stored: blank means none, line endings are Unix.
fn tidy(template: Option<&str>) -> Option<String> {
    template
        .filter(|text| !text.trim().is_empty())
        .map(|text| text.replace("\r\n", "\n"))
}

pub struct DispatcherPromptService {
    config: Arc<DispatcherConfigService>,
    tickets: Arc<TicketService>,
    questions: Arc<QuestionService>,
}

impl std::fmt::Debug for DispatcherPromptService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatcherPromptService")
            .finish_non_exhaustive()
    }
}

impl DispatcherPromptService {
    #[must_use]
    pub fn new(
        config: Arc<DispatcherConfigService>,
        tickets: Arc<TicketService>,
        questions: Arc<QuestionService>,
    ) -> Self {
        Self {
            config,
            tickets,
            questions,
        }
    }

    /// Every kind with its default and own template.
    pub async fn list(&self) -> Result<PromptList, AppError> {
        let view = self.config.get().await?;
        Ok(PromptList {
            items: PromptKind::ALL
                .iter()
                .map(|kind| PromptView::of(&view.config, *kind))
                .collect(),
            version: view.version,
        })
    }

    pub async fn get(&self, kind: PromptKind) -> Result<PromptState, AppError> {
        let view = self.config.get().await?;
        Ok(state(&view, kind))
    }

    /// Stores an own template; a blank one goes back to the default.
    pub async fn set(
        &self,
        kind: PromptKind,
        template: &str,
        expected: Version,
    ) -> Result<PromptState, AppError> {
        let template = tidy(Some(template));
        if let Some(text) = &template {
            let fields = template_problems(kind, text);
            if !fields.is_empty() {
                return Err(invalid_template(fields));
            }
        }
        self.store(kind, template, expected).await
    }

    /// Drops the own template; the default applies again.
    pub async fn reset(
        &self,
        kind: PromptKind,
        expected: Version,
    ) -> Result<PromptState, AppError> {
        self.store(kind, None, expected).await
    }

    async fn store(
        &self,
        kind: PromptKind,
        template: Option<String>,
        expected: Version,
    ) -> Result<PromptState, AppError> {
        let mut config = self.config.get().await?.config;
        config.prompts.set(kind, template);
        let saved = self.config.update(config, expected).await?;
        Ok(state(&saved, kind))
    }

    /// Renders a template for a ticket, a question or sample values; nothing is saved.
    pub async fn preview(&self, request: &PreviewRequest) -> Result<PromptPreview, AppError> {
        let kind = request.kind;
        let draft = tidy(request.template.as_deref());
        if let Some(text) = &draft {
            let fields = template_problems(kind, text);
            if !fields.is_empty() {
                return Err(invalid_template(fields));
            }
        }
        let config = self.config.get().await?.config;
        let own = draft.as_deref().or_else(|| config.prompts.get(kind));
        let key = request
            .key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty());
        let sample = key.is_none();
        let (run, key, stand_ins) = match key {
            None => (sample_run(kind, &config), None, Vec::new()),
            Some(key) => self.run_for(kind, key, &config).await?,
        };
        let prompt = render_prompt(&run, &config, own).map_err(AppError::internal)?;
        Ok(PromptPreview {
            kind,
            key,
            sample,
            prompt,
            stand_ins,
        })
    }

    /// The instruction of a work run on a ticket that works on `branch`.
    pub async fn work_prompt(&self, key: &str, branch: &str) -> Result<String, AppError> {
        let config = self.config.get().await?.config;
        let detail = self.tickets.get(key).await?;
        let record = detail.view.ticket.record();
        let ticket = record.key.to_string();
        let feedback = feedback_of(&detail);
        let run = PromptRun::Work(WorkRun {
            agent: config.work_agent(&ticket),
            autopilot: config.autopilot_applies(&record.labels),
            title: record.title.as_str().to_owned(),
            answers: self.answers(&record.key, record.project).await?,
            branch: branch.to_owned(),
            feedback,
            ticket,
        });
        render_prompt(&run, &config, config.prompts.get(PromptKind::Work))
            .map_err(AppError::internal)
    }

    /// The instruction of a merge session; `previous` reports the failed earlier ones.
    pub async fn merge_prompt(
        &self,
        key: &str,
        branch: &str,
        target: &str,
        previous: Option<String>,
    ) -> Result<String, AppError> {
        let config = self.config.get().await?.config;
        let detail = self.tickets.get(key).await?;
        let record = detail.view.ticket.record();
        let ticket = record.key.to_string();
        let run = PromptRun::Merge(MergeRun {
            agent: config.work_agent(&ticket),
            title: record.title.as_str().to_owned(),
            branch: branch.to_owned(),
            target: target.to_owned(),
            previous_attempts: previous,
            ticket,
        });
        render_prompt(&run, &config, config.prompts.get(PromptKind::Merge))
            .map_err(AppError::internal)
    }

    /// The instruction of a plan run on a ticket.
    pub async fn plan_prompt(&self, key: &str) -> Result<String, AppError> {
        let config = self.config.get().await?.config;
        let detail = self.tickets.get(key).await?;
        let record = detail.view.ticket.record();
        let ticket = record.key.to_string();
        let run = PromptRun::Plan(PlanRun {
            agent: format!("{}{ticket}", nullradix_domain::PLAN_AGENT_PREFIX),
            title: record.title.as_str().to_owned(),
            ticket,
        });
        render_prompt(&run, &config, config.prompts.get(PromptKind::Plan))
            .map_err(AppError::internal)
    }

    /// The instruction of a research run on a question.
    pub async fn research_prompt(&self, key: &str) -> Result<String, AppError> {
        let config = self.config.get().await?.config;
        let detail = self.questions.get(key).await?;
        let record = detail.view.row.question.record();
        let key = record.key.to_string();
        let run = PromptRun::Research(ResearchRun {
            agent: format!("{}{key}", nullradix_domain::RESEARCH_AGENT_PREFIX),
            title: record.title.as_str().to_owned(),
            question: key,
        });
        render_prompt(&run, &config, config.prompts.get(PromptKind::Research))
            .map_err(AppError::internal)
    }

    async fn run_for(
        &self,
        kind: PromptKind,
        key: &str,
        config: &DispatcherConfig,
    ) -> Result<(PromptRun, Option<String>, Vec<&'static str>), AppError> {
        if kind == PromptKind::Research {
            let detail = self.questions.get(key).await?;
            let record = detail.view.row.question.record();
            let key = record.key.to_string();
            let run = PromptRun::Research(ResearchRun {
                agent: format!("{}{key}", nullradix_domain::RESEARCH_AGENT_PREFIX),
                title: record.title.as_str().to_owned(),
                question: key.clone(),
            });
            return Ok((run, Some(key), Vec::new()));
        }
        let detail = self.tickets.get(key).await?;
        let record = detail.view.ticket.record();
        let ticket = record.key.to_string();
        let title = record.title.as_str().to_owned();
        let branch = record
            .branch
            .clone()
            .unwrap_or_else(|| ticket_branch(&record.key, &title).as_str().to_owned());
        let (run, stand_ins) = match kind {
            PromptKind::Work => {
                let answers = self.answers(&record.key, record.project).await?;
                let feedback = feedback_of(&detail);
                let run = WorkRun {
                    agent: config.work_agent(&ticket),
                    autopilot: config.autopilot_applies(&record.labels),
                    ticket: ticket.clone(),
                    title,
                    branch,
                    answers,
                    feedback,
                };
                (PromptRun::Work(run), Vec::new())
            }
            PromptKind::Plan => (
                PromptRun::Plan(PlanRun {
                    agent: format!("{}{ticket}", nullradix_domain::PLAN_AGENT_PREFIX),
                    ticket: ticket.clone(),
                    title,
                }),
                Vec::new(),
            ),
            _ => (
                PromptRun::Merge(MergeRun {
                    agent: config.work_agent(&ticket),
                    target: record.target_branch.as_ref().map_or_else(
                        || DEFAULT_TARGET.to_owned(),
                        |target| target.as_str().to_owned(),
                    ),
                    ticket: ticket.clone(),
                    title,
                    branch,
                    previous_attempts: Some(ATTEMPTS_STAND_IN.to_owned()),
                }),
                vec!["previousAttempts"],
            ),
        };
        Ok((run, Some(ticket), stand_ins))
    }

    /// The decided questions of a ticket, as the next run should hear them.
    async fn answers(
        &self,
        ticket: &TicketKey,
        project: nullradix_domain::Id<nullradix_domain::Project>,
    ) -> Result<Vec<AnsweredQuestion>, AppError> {
        let filter = QuestionFilter {
            state: Some(QuestionState::Answered),
            ticket: Some(ticket.clone()),
            ..QuestionFilter::default()
        };
        let list = self.questions.list(project, &filter).await?;
        let mut answers = Vec::new();
        for view in list.questions {
            let key = view.row.question.record().key.to_string();
            let detail = self.questions.get(&key).await?;
            let record = detail.view.row.question.record();
            let Some(decision) = &record.decision else {
                continue;
            };
            answers.push(AnsweredQuestion {
                key,
                title: record.title.as_str().to_owned(),
                decision: describe(record.state, decision, detail.newest_options()),
            });
        }
        Ok(answers)
    }
}

fn state(view: &ConfigView, kind: PromptKind) -> PromptState {
    PromptState {
        prompt: PromptView::of(&view.config, kind),
        version: view.version,
    }
}

fn sample_run(kind: PromptKind, config: &DispatcherConfig) -> PromptRun {
    match kind {
        PromptKind::Work => PromptRun::Work(WorkRun {
            ticket: SAMPLE_TICKET.to_owned(),
            title: SAMPLE_TITLE.to_owned(),
            agent: config.work_agent(SAMPLE_TICKET),
            branch: String::from("ticket/ARC-0042-sample"),
            answers: vec![AnsweredQuestion {
                key: SAMPLE_QUESTION.to_owned(),
                title: String::from("Sample question: which variant?"),
                decision: String::from("option O2 \"B\" chosen. Reason: less effort"),
            }],
            feedback: vec![String::from(
                "Sample feedback from the review: please add a test for the error case.",
            )],
            autopilot: config.autopilot_applies(&[]),
        }),
        PromptKind::Plan => PromptRun::Plan(PlanRun {
            ticket: SAMPLE_TICKET.to_owned(),
            title: SAMPLE_TITLE.to_owned(),
            agent: format!("{}{SAMPLE_TICKET}", nullradix_domain::PLAN_AGENT_PREFIX),
        }),
        PromptKind::Research => PromptRun::Research(ResearchRun {
            question: SAMPLE_QUESTION.to_owned(),
            title: String::from("Sample question: which variant?"),
            agent: format!(
                "{}{SAMPLE_QUESTION}",
                nullradix_domain::RESEARCH_AGENT_PREFIX
            ),
        }),
        PromptKind::Merge => PromptRun::Merge(MergeRun {
            ticket: SAMPLE_TICKET.to_owned(),
            title: SAMPLE_TITLE.to_owned(),
            agent: config.work_agent(SAMPLE_TICKET),
            branch: String::from("ticket/ARC-0042-sample"),
            target: DEFAULT_TARGET.to_owned(),
            previous_attempts: Some(String::from(
                "Earlier merge sessions failed (session 1 of 3). Last report of the merge agent: \"Sample: conflict in the configuration could not be resolved.\"",
            )),
        }),
    }
}
