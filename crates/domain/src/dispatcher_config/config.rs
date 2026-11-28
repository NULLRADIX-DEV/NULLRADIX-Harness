use crate::ActorKind;

use super::values::{
    AccountStrategy, AutopilotSettings, ClaudeSettings, DEFAULT_ALLOWED_TOOLS, JoinWorktrees,
    MergeStrategy, PlanningSettings, Profile, PromptTemplates, ResearchSettings, Rule,
    RuleCondition, TimeWindow, WorktreeSetupStep,
};

/// Name of the profile used when a configuration lists none.
pub const FALLBACK_PROFILE: &str = "medium";

/// The resolved configuration of the dispatcher; `normalized` turns a draft into one.
#[allow(
    clippy::struct_excessive_bools,
    reason = "one switch per stored setting"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatcherConfig {
    /// Off means nothing new starts; running runs are still evaluated.
    pub enabled: bool,
    pub wip: i64,
    pub window: Option<TimeWindow>,
    pub opt_in_label: Option<String>,
    pub opt_out_label: Option<String>,
    pub agent_prefix: String,
    pub stale_minutes: i64,
    pub max_attempts: i64,
    pub pause_minutes_on_limit: i64,
    pub prompt_extra: String,
    pub prompts: PromptTemplates,
    pub merge_strategy: MergeStrategy,
    pub review_before_merge: bool,
    pub join_worktrees: JoinWorktrees,
    pub worktree_setup: Vec<WorktreeSetupStep>,
    pub worktree_setup_timeout_minutes: i64,
    pub claude: ClaudeSettings,
    /// In escalation order, cheapest first.
    pub profiles: Vec<Profile>,
    pub default_profile: String,
    pub rules: Vec<Rule>,
    pub escalate: bool,
    /// Profile names from cheap to expensive.
    pub ladder: Vec<String>,
    pub allow_fable: bool,
    pub research: ResearchSettings,
    pub planning: PlanningSettings,
    pub autopilot: AutopilotSettings,
    pub account_strategy: AccountStrategy,
    pub account_limit_percent: i64,
    pub account_reserve_five_hour: i64,
    pub account_reserve_week: i64,
    pub account_reset_grace_minutes: i64,
}

fn profile(name: &str, model: &str, effort: &str, turns: i64, minutes: i64) -> Profile {
    Profile {
        id: None,
        name: name.to_owned(),
        model: Some(model.to_owned()),
        effort: Some(effort.to_owned()),
        max_turns: turns,
        timeout_minutes: minutes,
        accounts: Vec::new(),
    }
}

fn rule(when: RuleCondition, profile: &str) -> Rule {
    Rule {
        id: None,
        when,
        profile: profile.to_owned(),
        accounts: Vec::new(),
    }
}

fn labels(names: &[&str]) -> RuleCondition {
    RuleCondition {
        labels: Some(names.iter().map(|name| (*name).to_owned()).collect()),
        ..RuleCondition::default()
    }
}

impl Default for ClaudeSettings {
    fn default() -> Self {
        Self {
            command: String::from("claude"),
            model: Some(String::from("sonnet")),
            effort: None,
            max_turns: 80,
            timeout_minutes: 60,
            permission_mode: Some(String::from("acceptEdits")),
            allowed_tools: DEFAULT_ALLOWED_TOOLS.map(String::from).to_vec(),
            use_subscription: true,
            extra_args: Vec::new(),
            git_bash_path: None,
        }
    }
}

/// What a fresh installation starts with: the dispatcher off, three profiles and four rules.
impl Default for DispatcherConfig {
    fn default() -> Self {
        let profiles = vec![
            profile("small", "sonnet", "high", 40, 45),
            profile("medium", "opus", "high", 80, 90),
            profile("large", "opus", "max", 150, 120),
        ];
        let ladder = profiles.iter().map(|found| found.name.clone()).collect();
        let urgent = RuleCondition {
            priorities: Some(vec![String::from("urgent")]),
            ..RuleCondition::default()
        };
        let many_files = RuleCondition {
            files_min: Some(6),
            ..RuleCondition::default()
        };
        Self {
            enabled: false,
            wip: 1,
            window: None,
            opt_in_label: Some(String::from("auto")),
            opt_out_label: Some(String::from("manual")),
            agent_prefix: String::from("worker-"),
            stale_minutes: 150,
            max_attempts: 2,
            pause_minutes_on_limit: 60,
            prompt_extra: String::new(),
            prompts: PromptTemplates::default(),
            merge_strategy: MergeStrategy::Merge,
            review_before_merge: true,
            join_worktrees: JoinWorktrees::None,
            worktree_setup: Vec::new(),
            worktree_setup_timeout_minutes: 15,
            claude: ClaudeSettings::default(),
            profiles,
            default_profile: String::from(FALLBACK_PROFILE),
            rules: vec![
                rule(labels(&["docs", "typo", "small"]), "small"),
                rule(labels(&["architecture", "refactoring"]), "large"),
                rule(urgent, "large"),
                rule(many_files, "large"),
            ],
            escalate: true,
            ladder,
            allow_fable: false,
            research: ResearchSettings {
                auto: false,
                wip: 1,
                max_per_day: 10,
                profile: None,
            },
            planning: PlanningSettings {
                auto: false,
                wip: 1,
                acceptance: true,
                profile: None,
            },
            autopilot: AutopilotSettings {
                enabled: false,
                except_label: Some(String::from("interactive")),
                max_merge_sessions: 3,
            },
            account_strategy: AccountStrategy::MostFree,
            account_limit_percent: 90,
            account_reserve_five_hour: 10,
            account_reserve_week: 2,
            account_reset_grace_minutes: 30,
        }
    }
}

impl DispatcherConfig {
    /// Every account slug the profiles and rules name, with the field it stands in.
    #[must_use]
    pub fn account_references(&self) -> Vec<(String, &str)> {
        let profiles = self.profiles.iter().enumerate().flat_map(|(at, found)| {
            found.accounts.iter().enumerate().map(move |(index, slug)| {
                (format!("profiles[{at}].accounts[{index}]"), slug.as_str())
            })
        });
        let rules =
            self.rules.iter().enumerate().flat_map(|(at, found)| {
                found.accounts.iter().enumerate().map(move |(index, slug)| {
                    (format!("rules[{at}].accounts[{index}]"), slug.as_str())
                })
            });
        profiles.chain(rules).collect()
    }

    /// Command fields with a different value, named as the API names them.
    #[must_use]
    pub fn command_fields_changed(&self, other: &Self) -> Vec<&'static str> {
        let (own, theirs) = (&self.claude, &other.claude);
        [
            ("claude.command", own.command != theirs.command),
            ("claude.extraArgs", own.extra_args != theirs.extra_args),
            (
                "claude.permissionMode",
                own.permission_mode != theirs.permission_mode,
            ),
            (
                "claude.allowedTools",
                own.allowed_tools != theirs.allowed_tools,
            ),
            (
                "claude.gitBashPath",
                own.git_bash_path != theirs.git_bash_path,
            ),
            ("worktreeSetup", self.worktree_setup != other.worktree_setup),
        ]
        .into_iter()
        .filter_map(|(field, changed)| changed.then_some(field))
        .collect()
    }

    /// The configuration without the row ids, to compare drafts with what is stored.
    #[must_use]
    pub fn without_ids(mut self) -> Self {
        self.profiles
            .iter_mut()
            .for_each(|profile| profile.id = None);
        self.rules.iter_mut().for_each(|rule| rule.id = None);
        self
    }

    #[must_use]
    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|found| found.name == name)
    }
}

/// Prefix of the agent that plans a ticket.
pub const PLAN_AGENT_PREFIX: &str = "planner-";
/// Prefix of the agent that researches a question.
pub const RESEARCH_AGENT_PREFIX: &str = "research-";

impl DispatcherConfig {
    /// Name of the agent of a work or merge run on a ticket.
    #[must_use]
    pub fn work_agent(&self, ticket_key: &str) -> String {
        format!("{}{ticket_key}", self.agent_prefix)
    }

    /// Whether an agent of the dispatcher asked: not a person, and named as a work or plan agent.
    #[must_use]
    pub fn asked_by_dispatch_agent(&self, kind: ActorKind, name: &str) -> bool {
        if kind == ActorKind::Human {
            return false;
        }
        let work = !self.agent_prefix.is_empty() && name.starts_with(&self.agent_prefix);
        work || name.starts_with(PLAN_AGENT_PREFIX)
    }

    /// Whether autopilot applies to a ticket with these labels.
    #[must_use]
    pub fn autopilot_applies(&self, labels: &[String]) -> bool {
        if !self.autopilot.enabled {
            return false;
        }
        match self.autopilot.except_label.as_deref().map(str::trim) {
            Some(except) if !except.is_empty() => {
                !labels.iter().any(|label| crate::same_label(label, except))
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn autopilot_follows_the_switch_and_the_except_label() {
        let mut config = DispatcherConfig::default();
        assert!(!config.autopilot_applies(&[]));
        config.autopilot.enabled = true;
        assert!(config.autopilot_applies(&labels(&["docs"])));
        assert!(!config.autopilot_applies(&labels(&["docs", " Interactive "])));
        config.autopilot.except_label = None;
        assert!(config.autopilot_applies(&labels(&["interactive"])));
    }

    #[test]
    fn only_agents_of_the_dispatcher_count_as_its_askers() {
        let mut config = DispatcherConfig::default();
        for name in ["worker-ARC-1", "planner-ARC-1"] {
            assert!(
                config.asked_by_dispatch_agent(ActorKind::Session, name),
                "{name}"
            );
        }
        assert!(!config.asked_by_dispatch_agent(ActorKind::Human, "worker-ARC-1"));
        for name in ["claude", "research-ARC-Q1", "workerARC-1"] {
            assert!(
                !config.asked_by_dispatch_agent(ActorKind::Session, name),
                "{name}"
            );
        }
        config.agent_prefix = String::new();
        assert!(!config.asked_by_dispatch_agent(ActorKind::Session, "claude"));
        assert!(config.asked_by_dispatch_agent(ActorKind::Session, "planner-ARC-1"));
    }

    #[test]
    fn agents_are_named_after_their_run() {
        let config = DispatcherConfig::default();
        assert_eq!(config.work_agent("ARC-9"), "worker-ARC-9");
    }
}
