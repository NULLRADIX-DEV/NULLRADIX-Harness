//! What the dispatcher records about a run beside the journal: its purpose, settings and usage.

use strum::{EnumString, IntoStaticStr};
use uuid::Uuid;

/// What a run does; `work` implements a ticket, `merge` resolves a merge conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum RunKind {
    Work,
    Merge,
    Research,
    Plan,
}

impl RunKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// Research and plan runs get a fixed, read-mostly tool set.
    #[must_use]
    pub const fn is_restricted(self) -> bool {
        matches!(self, Self::Research | Self::Plan)
    }

    /// Research runs belong to a question, every other kind to a ticket.
    #[must_use]
    pub const fn is_about_question(self) -> bool {
        matches!(self, Self::Research)
    }
}

/// Who started a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum RunTrigger {
    Dispatcher,
    Manual,
}

impl RunTrigger {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Why a run did not end well, as far as the output and the exit tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum AbortCategory {
    Timeout,
    MaxTurns,
    Context,
    ExitCode,
    ClaudeError,
    UsageLimit,
    StartFailed,
    Vanished,
}

impl AbortCategory {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// How the dispatcher settled a finished run; the stored values are checked by the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum DispatchOutcome {
    Done,
    Review,
    Human,
    Abort,
    AbortFinal,
    Blocked,
    Question,
    PauseLimit,
    PauseClaudeMissing,
    SetupFailed,
    Taken,
    Gone,
    Other,
    MergeSession,
    Research,
    Planned,
}

impl DispatchOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// The reason a run counts as aborted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Abort {
    pub category: AbortCategory,
    pub reason: String,
}

/// The profile a run started with, kept as it was even when the profile changes later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunProfile {
    /// The stored profile; gone when the profile was deleted.
    pub id: Option<Uuid>,
    pub name: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub max_turns: u32,
    pub timeout_minutes: u32,
}

/// Settings and subject of a Claude run, fixed when it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRun {
    pub kind: RunKind,
    pub trigger: RunTrigger,
    /// Ticket or question key; kept when the subject is deleted.
    pub subject_key: Option<String>,
    pub profile: RunProfile,
    /// The account whose folder the run used, when one was chosen.
    pub account: Option<Uuid>,
    /// Hash of the dispatcher configuration in force.
    pub config_hash: String,
    /// The agent name the run worked under; none for runs from before it was kept.
    pub agent: Option<String>,
    pub cwd: String,
    /// Arguments after the program, as the run was started.
    pub args: Vec<String>,
    pub prompt: String,
}

/// Tokens and cost of one model within a run.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
}

/// Cost and tokens of a run as Claude reported them in its result.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunUsage {
    pub cost_usd: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub num_turns: Option<i64>,
    /// The model that cost the most.
    pub model: Option<String>,
    pub models: Vec<ModelUsage>,
}

/// A tool Claude Code refused to run, with how often.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeniedTool {
    pub tool: String,
    pub count: u32,
}

/// What the run recorded after its end.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunResult {
    pub session_id: Option<String>,
    pub usage: Option<RunUsage>,
    /// Tool calls Claude Code refused; none without a result to count them in.
    pub denied_count: Option<u32>,
    pub abort: Option<Abort>,
    /// Set once the dispatcher has settled the run; a finished run without one is still open.
    pub outcome: Option<DispatchOutcome>,
}

/// Where a screenshot of a run lies on its runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screenshot {
    /// The tool call that took it.
    pub tool_use_id: Option<String>,
    pub media_type: String,
    /// File name inside the run's artifact folder.
    pub file: String,
    pub bytes: u64,
}
