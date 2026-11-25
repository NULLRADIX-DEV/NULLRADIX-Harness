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
