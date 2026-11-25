use super::values::PromptKind;

pub(super) const LINK_RULE: &str = include_str!("prompts/link_rule.txt");
pub(super) const TOOL_RULE: &str = include_str!("prompts/tool_rule.txt");
/// Outcomes of a work run; it holds `{{ticketId}}` and `{{agent}}` itself.
pub(super) const OUTCOMES: &str = include_str!("prompts/outcomes.txt");
/// Outcomes under autopilot: no Human Interaction, no acceptance by a person.
pub(super) const OUTCOMES_AUTOPILOT: &str = include_str!("prompts/outcomes_autopilot.txt");
pub(super) const ACCEPTANCE_RULE: &str = include_str!("prompts/acceptance_rule.txt");

const WORK_TEMPLATE: &str = include_str!("prompts/work.txt");
const PLAN_TEMPLATE: &str = include_str!("prompts/plan.txt");
const RESEARCH_TEMPLATE: &str = include_str!("prompts/research.txt");
const MERGE_TEMPLATE: &str = include_str!("prompts/merge.txt");

/// The template a kind of run uses without an own one.
#[must_use]
pub const fn default_template(kind: PromptKind) -> &'static str {
    match kind {
        PromptKind::Work => WORK_TEMPLATE,
        PromptKind::Plan => PLAN_TEMPLATE,
        PromptKind::Research => RESEARCH_TEMPLATE,
        PromptKind::Merge => MERGE_TEMPLATE,
    }
}

/// A placeholder a template may use and what goes there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaceholderInfo {
    pub name: &'static str,
    pub description: &'static str,
}

const fn info(name: &'static str, description: &'static str) -> PlaceholderInfo {
    PlaceholderInfo { name, description }
}

const AGENT: PlaceholderInfo = info("agent", "Name of the agent that does the run");
const TICKET_ID: PlaceholderInfo = info("ticketId", "Key of the ticket, such as ARC-9");
const QUESTION_ID: PlaceholderInfo = info("questionId", "Key of the question, such as ARC-Q7");
const TITLE: PlaceholderInfo = info("title", "Title of the ticket or the question");
const BRANCH: PlaceholderInfo = info("branch", "Branch of the ticket");
const LINK_RULE_INFO: PlaceholderInfo = info(
    "linkRule",
    "Rule against links to the main folder and other worktrees",
);
const TOOL_RULE_INFO: PlaceholderInfo = info("toolRule", "Rules for handling files and commands");
const PROMPT_EXTRA: PlaceholderInfo = info(
    "promptExtra",
    "Extra instruction from the dispatcher settings; empty when none is set",
);

const WORK_PLACEHOLDERS: [PlaceholderInfo; 11] = [
    AGENT,
    TICKET_ID,
    TITLE,
    BRANCH,
    info(
        "worktreeSetup",
        "Note on the setup the dispatcher runs; empty when no setup step is set",
    ),
    LINK_RULE_INFO,
    TOOL_RULE_INFO,
    info(
        "outcomes",
        "The possible outcomes of the run with their commands; without Human Interaction under autopilot",
    ),
    info(
        "answers",
        "Decided questions of this ticket; empty when there are none",
    ),
    info(
        "feedback",
        "Feedback from the review; empty when there is none",
    ),
    PROMPT_EXTRA,
];

const PLAN_PLACEHOLDERS: [PlaceholderInfo; 5] = [
    AGENT,
    TICKET_ID,
    TITLE,
    info(
        "acceptanceRule",
        "Instruction to write the acceptance into plan.json as well; empty when planning does not fill it",
    ),
    PROMPT_EXTRA,
];

const RESEARCH_PLACEHOLDERS: [PlaceholderInfo; 4] = [AGENT, QUESTION_ID, TITLE, PROMPT_EXTRA];

const MERGE_PLACEHOLDERS: [PlaceholderInfo; 9] = [
    AGENT,
    BRANCH,
    TICKET_ID,
    TITLE,
    info("target", "Target branch of the merge, such as main"),
    info(
        "previousAttempts",
        "Report of earlier failed merge sessions; empty for the first one",
    ),
    LINK_RULE_INFO,
    TOOL_RULE_INFO,
    PROMPT_EXTRA,
];

/// The placeholders a kind of run knows, in the order the editor lists them.
#[must_use]
pub fn placeholders(kind: PromptKind) -> &'static [PlaceholderInfo] {
    match kind {
        PromptKind::Work => &WORK_PLACEHOLDERS,
        PromptKind::Plan => &PLAN_PLACEHOLDERS,
        PromptKind::Research => &RESEARCH_PLACEHOLDERS,
        PromptKind::Merge => &MERGE_PLACEHOLDERS,
    }
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::super::prompt_check::{missing_parts, unknown_placeholders};
    use super::super::prompt_template::placeholder_names;
    use super::*;

    #[test]
    fn defaults_are_ascii_with_unix_line_endings() {
        for kind in PromptKind::iter() {
            let text = default_template(kind);
            assert!(text.is_ascii(), "{kind:?}");
            assert!(!text.contains('\r'), "{kind:?}");
        }
        for text in [
            LINK_RULE,
            TOOL_RULE,
            OUTCOMES,
            OUTCOMES_AUTOPILOT,
            ACCEPTANCE_RULE,
        ] {
            assert!(text.is_ascii() && !text.contains('\r'));
        }
    }

    #[test]
    fn defaults_use_only_known_placeholders_and_hold_every_required_part() {
        for kind in PromptKind::iter() {
            let text = default_template(kind);
            assert_eq!(unknown_placeholders(kind, text), Vec::<String>::new());
            assert!(missing_parts(kind, text).is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn every_listed_placeholder_is_used_by_the_default() {
        for kind in PromptKind::iter() {
            let used = placeholder_names(default_template(kind));
            for placeholder in placeholders(kind) {
                assert!(
                    used.iter().any(|name| name == placeholder.name),
                    "{kind:?} {}",
                    placeholder.name
                );
            }
        }
    }

    #[test]
    fn fragments_only_use_work_placeholders() {
        let allowed = ["ticketId", "agent"];
        for text in [OUTCOMES, OUTCOMES_AUTOPILOT] {
            for name in placeholder_names(text) {
                assert!(allowed.contains(&name.as_str()), "{name}");
            }
        }
    }
}
