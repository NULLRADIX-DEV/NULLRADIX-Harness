use super::config::DispatcherConfig;
use super::prompt_defaults::{
    ACCEPTANCE_RULE, LINK_RULE, OUTCOMES, OUTCOMES_AUTOPILOT, TOOL_RULE, default_template,
};
use super::prompt_template::{PromptValues, UnknownPlaceholders, render_template};
use super::values::PromptKind;

/// A decided question of the ticket the next run must know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsweredQuestion {
    pub key: String,
    pub title: String,
    /// The decision in words, with its reason.
    pub decision: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkRun {
    pub ticket: String,
    pub title: String,
    pub agent: String,
    pub branch: String,
    pub answers: Vec<AnsweredQuestion>,
    /// Review feedback since the last start.
    pub feedback: Vec<String>,
    /// No person accepts the result, so the run has no Human Interaction outcome.
    pub autopilot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRun {
    pub ticket: String,
    pub title: String,
    pub agent: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchRun {
    pub question: String,
    pub title: String,
    pub agent: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRun {
    pub ticket: String,
    pub title: String,
    pub agent: String,
    pub branch: String,
    pub target: String,
    /// Report of failed earlier merge sessions; none for the first.
    pub previous_attempts: Option<String>,
}

/// What a run is about, as far as its instruction needs to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptRun {
    Work(WorkRun),
    Plan(PlanRun),
    Research(ResearchRun),
    Merge(MergeRun),
}

impl PromptRun {
    #[must_use]
    pub const fn kind(&self) -> PromptKind {
        match self {
            Self::Work(_) => PromptKind::Work,
            Self::Plan(_) => PromptKind::Plan,
            Self::Research(_) => PromptKind::Research,
            Self::Merge(_) => PromptKind::Merge,
        }
    }
}

fn values(pairs: &[(&str, &str)]) -> PromptValues {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

/// The decided questions as a block; the blank line in front belongs to the value.
fn answers_block(answers: &[AnsweredQuestion]) -> String {
    if answers.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        String::new(),
        String::from(
            "Decisions on questions of this ticket (they take precedence over plan and handbook where they contradict them):",
        ),
    ];
    for answer in answers {
        lines.push(format!(
            "- {} \"{}\": {}",
            answer.key, answer.title, answer.decision
        ));
    }
    lines.join("\n")
}

/// The review feedback as a block; later lines of one entry are indented.
fn feedback_block(feedback: &[String]) -> String {
    if feedback.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        String::new(),
        String::from(
            "Feedback from the review (work instruction, takes precedence over plan and handbook):",
        ),
    ];
    for item in feedback {
        lines.push(format!("- {}", item.replace('\n', "\n  ")));
    }
    lines.push(String::from(
        "A person looked at your implementation and sent it back. Apply the feedback on the same branch, then finish with \"done\" again.",
    ));
    lines.join("\n")
}

fn previous_attempts_block(previous: Option<&str>) -> String {
    match previous.map(str::trim) {
        Some(text) if !text.is_empty() => format!("\n{text}"),
        _ => String::new(),
    }
}

/// The extra instruction with the blank line in front; empty when none is set.
fn prompt_extra(config: &DispatcherConfig) -> String {
    let extra = config.prompt_extra.trim();
    if extra.is_empty() {
        String::new()
    } else {
        format!("\n\n{extra}")
    }
}

fn worktree_setup_note(config: &DispatcherConfig) -> String {
    if config.worktree_setup.is_empty() {
        return String::new();
    }
    let steps: Vec<String> = config
        .worktree_setup
        .iter()
        .map(|step| format!("{}$ {}", step.cwd, step.run))
        .collect();
    format!(
        "- The dependencies are installed by the worktree setup of the dispatcher ({}).",
        steps.join("; ")
    )
}

fn work_values(
    run: &WorkRun,
    config: &DispatcherConfig,
) -> Result<PromptValues, UnknownPlaceholders> {
    let mut all = values(&[
        ("ticketId", &run.ticket),
        ("title", &run.title),
        ("agent", &run.agent),
        ("branch", &run.branch),
        ("linkRule", LINK_RULE),
        ("toolRule", TOOL_RULE),
    ]);
    all.insert("worktreeSetup".to_owned(), worktree_setup_note(config));
    all.insert("answers".to_owned(), answers_block(&run.answers));
    all.insert("feedback".to_owned(), feedback_block(&run.feedback));
    all.insert("promptExtra".to_owned(), prompt_extra(config));
    let outcomes = if run.autopilot {
        OUTCOMES_AUTOPILOT
    } else {
        OUTCOMES
    };
    let outcomes = render_template(outcomes, &all)?;
    all.insert("outcomes".to_owned(), outcomes);
    Ok(all)
}

fn run_values(
    run: &PromptRun,
    config: &DispatcherConfig,
) -> Result<PromptValues, UnknownPlaceholders> {
    let extra = prompt_extra(config);
    Ok(match run {
        PromptRun::Work(work) => work_values(work, config)?,
        PromptRun::Plan(plan) => {
            let acceptance = if config.planning.acceptance {
                ACCEPTANCE_RULE
            } else {
                ""
            };
            values(&[
                ("ticketId", &plan.ticket),
                ("title", &plan.title),
                ("agent", &plan.agent),
                ("acceptanceRule", acceptance),
                ("promptExtra", &extra),
            ])
        }
        PromptRun::Research(research) => values(&[
            ("questionId", &research.question),
            ("title", &research.title),
            ("agent", &research.agent),
            ("promptExtra", &extra),
        ]),
        PromptRun::Merge(merge) => values(&[
            ("ticketId", &merge.ticket),
            ("title", &merge.title),
            ("agent", &merge.agent),
            ("branch", &merge.branch),
            ("target", &merge.target),
            (
                "previousAttempts",
                &previous_attempts_block(merge.previous_attempts.as_deref()),
            ),
            ("linkRule", LINK_RULE),
            ("toolRule", TOOL_RULE),
            ("promptExtra", &extra),
        ]),
    })
}

/// Renders the instruction of a run with `template`; the default applies when it is none or fails.
pub fn render_prompt(
    run: &PromptRun,
    config: &DispatcherConfig,
    template: Option<&str>,
) -> Result<String, UnknownPlaceholders> {
    let all = run_values(run, config)?;
    if let Some(text) = template.and_then(|own| render_template(own, &all).ok()) {
        return Ok(text);
    }
    render_template(default_template(run.kind()), &all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorktreeSetupStep;

    fn config() -> DispatcherConfig {
        DispatcherConfig {
            prompt_extra: String::from("House rule: no any."),
            ..DispatcherConfig::default()
        }
    }

    fn work(answers: Vec<AnsweredQuestion>, feedback: Vec<String>, autopilot: bool) -> PromptRun {
        PromptRun::Work(WorkRun {
            ticket: String::from("ARC-9"),
            title: String::from("Discounts"),
            agent: String::from("worker-ARC-9"),
            branch: String::from("ticket/ARC-0009-discounts"),
            answers,
            feedback,
            autopilot,
        })
    }

    fn render(run: &PromptRun, config: &DispatcherConfig) -> String {
        render_prompt(run, config, config.prompts.get(run.kind())).unwrap()
    }

    #[test]
    fn a_work_prompt_without_extras_drops_the_empty_blocks() {
        let text = render(
            &work(Vec::new(), Vec::new(), false),
            &DispatcherConfig::default(),
        );
        assert!(text.starts_with("You are worker-ARC-9, a junior developer agent."));
        assert!(text.contains("nullradix ticket done ARC-9 --acceptance"));
        assert!(text.contains("Human Interaction: <what>"));
        assert!(text.contains("- Do not create links"));
        assert!(!text.contains("{{"));
        assert!(!text.contains("Decisions on questions"));
        assert!(!text.contains("dependencies are installed"));
        assert!(!text.contains("\n\n\n"));
        assert!(text.ends_with("without them."));
    }

    #[test]
    fn a_work_prompt_shows_answers_feedback_setup_and_the_extra() {
        let mut config = config();
        config.worktree_setup = vec![WorktreeSetupStep {
            cwd: String::from("web"),
            run: String::from("npm ci"),
        }];
        let answers = vec![AnsweredQuestion {
            key: String::from("ARC-Q7"),
            title: String::from("Which variant?"),
            decision: String::from("option O2 \"B\" chosen. Reason: cheaper"),
        }];
        let feedback = vec![String::from("Add a test\nfor the error case")];
        let text = render(&work(answers, feedback, false), &config);
        assert!(
            text.contains("- ARC-Q7 \"Which variant?\": option O2 \"B\" chosen. Reason: cheaper")
        );
        assert!(text.contains("- Add a test\n  for the error case"));
        assert!(text.contains("(web$ npm ci)"));
        assert!(text.ends_with("\n\nHouse rule: no any."));
    }

    #[test]
    fn autopilot_takes_the_human_interaction_outcome_away() {
        let text = render(&work(Vec::new(), Vec::new(), true), &config());
        assert!(!text.contains("Human Interaction: <what>"));
        assert!(text.contains("End with exactly one of three outcomes"));
    }

    #[test]
    fn plan_research_and_merge_fill_their_values() {
        let config = config();
        let plan = render(
            &PromptRun::Plan(PlanRun {
                ticket: String::from("ARC-7"),
                title: String::from("Plan me"),
                agent: String::from("planner-ARC-7"),
            }),
            &config,
        );
        assert!(plan.contains("nullradix plan import ARC-7 --file plan.json"));
        assert!(plan.contains("acceptance of the ticket"));
        let research = render(
            &PromptRun::Research(ResearchRun {
                question: String::from("ARC-Q7"),
                title: String::from("Which?"),
                agent: String::from("research-ARC-Q7"),
            }),
            &config,
        );
        assert!(research.contains("nullradix research submit ARC-Q7 --file research.json"));
        let merge = render(
            &PromptRun::Merge(MergeRun {
                ticket: String::from("ARC-9"),
                title: String::from("Discounts"),
                agent: String::from("worker-ARC-9"),
                branch: String::from("ticket/arc-9"),
                target: String::from("main"),
                previous_attempts: Some(String::from("  Failed once.  ")),
            }),
            &config,
        );
        assert!(merge.contains("4. Commit everything (merge commit).\n\nFailed once.\n\nRules:"));
    }

    #[test]
    fn planning_without_acceptance_drops_that_rule() {
        let mut config = DispatcherConfig::default();
        config.planning.acceptance = false;
        let text = render(
            &PromptRun::Plan(PlanRun {
                ticket: String::from("ARC-7"),
                title: String::from("t"),
                agent: String::from("a"),
            }),
            &config,
        );
        assert!(!text.contains("acceptance of the ticket"));
    }

    #[test]
    fn an_own_template_wins_and_a_broken_one_falls_back() {
        let run = work(Vec::new(), Vec::new(), false);
        let config = DispatcherConfig::default();
        let own = render_prompt(&run, &config, Some("Do {{ticketId}} as {{agent}}")).unwrap();
        assert_eq!(own, "Do ARC-9 as worker-ARC-9");
        let broken = render_prompt(&run, &config, Some("Do {{nope}}")).unwrap();
        assert_eq!(broken, render_prompt(&run, &config, None).unwrap());
    }

    #[test]
    fn every_default_renders_for_every_kind() {
        let config = DispatcherConfig::default();
        let runs = [
            work(Vec::new(), Vec::new(), false),
            PromptRun::Plan(PlanRun {
                ticket: String::from("A-1"),
                title: String::new(),
                agent: String::from("a"),
            }),
            PromptRun::Research(ResearchRun {
                question: String::from("A-Q1"),
                title: String::new(),
                agent: String::from("a"),
            }),
            PromptRun::Merge(MergeRun {
                ticket: String::from("A-1"),
                title: String::new(),
                agent: String::from("a"),
                branch: String::from("b"),
                target: String::from("main"),
                previous_attempts: None,
            }),
        ];
        for run in runs {
            let text = render_prompt(&run, &config, None).unwrap();
            assert!(!text.contains("{{"), "{:?}", run.kind());
        }
    }
}
