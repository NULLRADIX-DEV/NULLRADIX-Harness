use strum::{EnumIter, EnumString, IntoStaticStr};

/// Tools a work run may use without asking.
pub const DEFAULT_ALLOWED_TOOLS: [&str; 16] = [
    "Read",
    "Edit",
    "Write",
    "Glob",
    "Grep",
    "Bash(harness:*)",
    "Bash(nullradix:*)",
    "Bash(node:*)",
    "Bash(npm test:*)",
    "Bash(npm run:*)",
    "Bash(git status:*)",
    "Bash(git diff:*)",
    "Bash(git log:*)",
    "Bash(git branch:*)",
    "Bash(git add:*)",
    "Bash(git commit:*)",
];

/// Effort levels of Claude Code; which ones a model can do is up to Claude Code.
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Model aliases Claude Code knows; full names `claude-...` are accepted as well.
pub const MODEL_ALIASES: [&str; 11] = [
    "default",
    "sonnet",
    "opus",
    "haiku",
    "fable",
    "best",
    "opusplan",
    "sonnet[1m]",
    "opus[1m]",
    "opusplan[1m]",
    "fable[1m]",
];

pub const PROFILE_NAME_MAX: usize = 60;
pub const MAX_PARALLEL_AUTO_RUNS: i64 = 5;

/// How a finished ticket branch comes back: all commits or one squashed commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, EnumIter, EnumString, IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
pub enum MergeStrategy {
    #[default]
    Merge,
    Squash,
}

impl MergeStrategy {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// One worktree per ticket, or the tickets of one spec sharing a worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, EnumIter, EnumString, IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
pub enum JoinWorktrees {
    #[default]
    None,
    SameSpec,
}

impl JoinWorktrees {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// How an account is picked among the allowed ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, EnumIter, EnumString, IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
pub enum AccountStrategy {
    #[default]
    MostFree,
    Order,
}

impl AccountStrategy {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Kind of run a prompt template is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumString, IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
pub enum PromptKind {
    Work,
    Plan,
    Research,
    Merge,
}

impl PromptKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Own prompt templates per kind of run; none means the built-in template.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptTemplates {
    pub work: Option<String>,
    pub plan: Option<String>,
    pub research: Option<String>,
    pub merge: Option<String>,
}

impl PromptTemplates {
    #[must_use]
    pub fn get(&self, kind: PromptKind) -> Option<&str> {
        match kind {
            PromptKind::Work => self.work.as_deref(),
            PromptKind::Plan => self.plan.as_deref(),
            PromptKind::Research => self.research.as_deref(),
            PromptKind::Merge => self.merge.as_deref(),
        }
    }

    pub fn set(&mut self, kind: PromptKind, template: Option<String>) {
        match kind {
            PromptKind::Work => self.work = template,
            PromptKind::Plan => self.plan = template,
            PromptKind::Research => self.research = template,
            PromptKind::Merge => self.merge = template,
        }
    }

    /// The set templates in the order work, plan, research, merge.
    #[must_use]
    pub fn entries(&self) -> Vec<(PromptKind, &str)> {
        [
            PromptKind::Work,
            PromptKind::Plan,
            PromptKind::Research,
            PromptKind::Merge,
        ]
        .into_iter()
        .filter_map(|kind| self.get(kind).map(|template| (kind, template)))
        .collect()
    }
}

/// Local time window in which new runs may start; `to` before `from` wraps past midnight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeWindow {
    /// `HH:MM`, inclusive.
    pub from: String,
    /// `HH:MM`, exclusive.
    pub to: String,
}

/// How the dispatcher calls Claude Code and the limits it applies by default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSettings {
    pub command: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub max_turns: i64,
    pub timeout_minutes: i64,
    pub permission_mode: Option<String>,
    pub allowed_tools: Vec<String>,
    pub use_subscription: bool,
    pub extra_args: Vec<String>,
    pub git_bash_path: Option<String>,
}

/// Model, effort and limits of a run; the order of profiles is the escalation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub max_turns: i64,
    pub timeout_minutes: i64,
    /// Account slugs the profile may use in this order; none means any enabled account.
    pub accounts: Vec<String>,
}

/// Conditions of a rule; all that are set must hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleCondition {
    pub labels: Option<Vec<String>>,
    pub priorities: Option<Vec<String>>,
    pub files_min: Option<i64>,
    pub plan_min_chars: Option<i64>,
}

impl RuleCondition {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.labels.is_none()
            && self.priorities.is_none()
            && self.files_min.is_none()
            && self.plan_min_chars.is_none()
    }
}

/// When `when` holds, `profile` applies; the first matching rule wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub when: RuleCondition,
    pub profile: String,
    /// Replaces the accounts of the profile when set.
    pub accounts: Vec<String>,
}

/// Research runs on open questions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchSettings {
    pub auto: bool,
    pub wip: i64,
    pub max_per_day: i64,
    pub profile: Option<String>,
}

/// Planning runs on tickets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanningSettings {
    pub auto: bool,
    pub wip: i64,
    pub acceptance: bool,
    pub profile: Option<String>,
}

/// Autopilot: no approvals, research answers questions, Claude resolves merge conflicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutopilotSettings {
    pub enabled: bool,
    pub except_label: Option<String>,
    pub max_merge_sessions: i64,
}

/// Command run in a fresh worktree before Claude starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeSetupStep {
    /// Folder relative to the worktree root; `.` is the root itself.
    pub cwd: String,
    pub run: String,
}
