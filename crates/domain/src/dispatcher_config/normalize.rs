use std::collections::HashSet;
use std::fmt;

use crate::{AccountSlug, TextError, required_text};

use super::config::{DispatcherConfig, FALLBACK_PROFILE};
use super::prompt_check::{missing_parts, size_problem, unknown_placeholders};
use super::values::{
    ClaudeSettings, EFFORT_LEVELS, MAX_PARALLEL_AUTO_RUNS, MODEL_ALIASES, PROFILE_NAME_MAX,
    Profile, PromptKind, Rule, TimeWindow, WorktreeSetupStep,
};

/// A refused value and the field it was sent in, such as `profiles[1].maxTurns`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFieldError {
    pub field: String,
    pub message: String,
}

/// Every field a draft configuration got wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub Vec<ConfigFieldError>);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .0
            .iter()
            .map(|error| format!("{}: {}", error.field, error.message))
            .collect();
        write!(f, "invalid dispatcher configuration: {}", parts.join("; "))
    }
}

impl std::error::Error for ConfigError {}

#[derive(Default)]
struct Checker {
    errors: Vec<ConfigFieldError>,
}

impl Checker {
    fn add(&mut self, field: &str, message: impl Into<String>) {
        self.errors.push(ConfigFieldError {
            field: field.to_owned(),
            message: message.into(),
        });
    }

    /// Records a value outside `min..=max`.
    fn int(&mut self, field: &str, value: i64, min: i64, max: Option<i64>) {
        let inside = value >= min && max.is_none_or(|max| value <= max);
        if !inside {
            let range = max.map_or_else(
                || format!("from {min}"),
                |max| format!("from {min} to {max}"),
            );
            self.add(field, format!("must be a whole number {range}"));
        }
    }
}

fn trimmed(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn tidy(text: &mut String) {
    let kept = text.trim().to_owned();
    text.clone_from(&kept);
}

fn trimmed_option(text: Option<&str>) -> Option<String> {
    text.and_then(trimmed)
}

fn is_clock(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digit = |at: usize| bytes.get(at).is_some_and(u8::is_ascii_digit);
    let number = |at: usize| {
        let tens = i32::from(bytes[at] - b'0');
        tens * 10 + i32::from(bytes[at + 1] - b'0')
    };
    bytes.len() == 5
        && digit(0)
        && digit(1)
        && bytes[2] == b':'
        && digit(3)
        && digit(4)
        && number(0) <= 23
        && number(3) <= 59
}

/// Why a model name is refused, or none.
#[must_use]
pub fn model_problem(model: &str, allow_fable: bool) -> Option<String> {
    let full = model.strip_suffix("[1m]").unwrap_or(model);
    let full_name = full.strip_prefix("claude-").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
    });
    if !MODEL_ALIASES.contains(&model) && !full_name {
        return Some(format!(
            "unknown model \"{model}\" (an alias such as sonnet, opus or haiku, or a full name claude-...)"
        ));
    }
    let fable = model.to_ascii_lowercase().contains("fable") || model == "best";
    (fable && !allow_fable).then(|| {
        format!(
            "Fable (\"{model}\") is not allowed: headless runs may bill usage credits without asking, allow it first"
        )
    })
}

/// Why an effort level is refused, or none.
#[must_use]
pub fn effort_problem(effort: &str) -> Option<String> {
    (!EFFORT_LEVELS.contains(&effort))
        .then(|| format!("unknown effort \"{effort}\" ({})", EFFORT_LEVELS.join(", ")))
}

fn model_and_effort(
    field: &str,
    model: Option<&str>,
    effort: Option<&str>,
    allow_fable: bool,
    check: &mut Checker,
) {
    if let Some(problem) = model.and_then(|model| model_problem(model, allow_fable)) {
        check.add(&format!("{field}.model"), problem);
    }
    if let Some(problem) = effort.and_then(effort_problem) {
        check.add(&format!("{field}.effort"), problem);
    }
}

fn claude(settings: &mut ClaudeSettings, allow_fable: bool, check: &mut Checker) {
    settings.command = trimmed(&settings.command).unwrap_or_else(|| String::from("claude"));
    settings.model = trimmed_option(settings.model.as_deref());
    settings.effort = trimmed_option(settings.effort.as_deref());
    settings.permission_mode = trimmed_option(settings.permission_mode.as_deref());
    settings.git_bash_path = trimmed_option(settings.git_bash_path.as_deref());
    model_and_effort(
        "claude",
        settings.model.as_deref(),
        settings.effort.as_deref(),
        allow_fable,
        check,
    );
    check.int("claude.maxTurns", settings.max_turns, 1, None);
    check.int("claude.timeoutMinutes", settings.timeout_minutes, 1, None);
    settings.allowed_tools = settings
        .allowed_tools
        .iter()
        .filter_map(|tool| trimmed(tool))
        .collect();
}

/// The slugs without blanks and repeats; a slug that is no account id is an error.
fn accounts(slugs: &[String], field: &str, check: &mut Checker) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for (index, slug) in slugs.iter().enumerate() {
        let slug = slug.trim();
        if AccountSlug::parse(slug).is_err() {
            check.add(
                &format!("{field}[{index}]"),
                format!("\"{slug}\" is not an account id"),
            );
        } else if !kept.iter().any(|seen| seen == slug) {
            kept.push(slug.to_owned());
        }
    }
    kept
}

fn profiles(config: &mut DispatcherConfig, check: &mut Checker) {
    if config.profiles.is_empty() {
        config.profiles.push(Profile {
            id: None,
            name: String::from(FALLBACK_PROFILE),
            model: config.claude.model.clone(),
            effort: config.claude.effort.clone(),
            max_turns: config.claude.max_turns,
            timeout_minutes: config.claude.timeout_minutes,
            accounts: Vec::new(),
        });
    }
    let allow_fable = config.allow_fable;
    let mut ids = HashSet::new();
    let mut names: HashSet<String> = HashSet::new();
    for (at, profile) in config.profiles.iter_mut().enumerate() {
        let field = format!("profiles[{at}]");
        if profile.id.is_some_and(|id| !ids.insert(id)) {
            check.add(&format!("{field}.id"), "is used by another profile");
        }
        match required_text(&profile.name, PROFILE_NAME_MAX).map(str::to_owned) {
            Ok(name) => {
                profile.name = name;
                if !names.insert(profile.name.clone()) {
                    check.add(&format!("{field}.name"), "is used by another profile");
                }
            }
            Err(TextError::Empty) => check.add(&format!("{field}.name"), "is empty"),
            Err(TextError::TooLong(max)) => check.add(
                &format!("{field}.name"),
                format!("has more than {max} characters"),
            ),
        }
        profile.model = trimmed_option(profile.model.as_deref());
        profile.effort = trimmed_option(profile.effort.as_deref());
        model_and_effort(
            &field,
            profile.model.as_deref(),
            profile.effort.as_deref(),
            allow_fable,
            check,
        );
        check.int(&format!("{field}.maxTurns"), profile.max_turns, 1, None);
        check.int(
            &format!("{field}.timeoutMinutes"),
            profile.timeout_minutes,
            1,
            None,
        );
        profile.accounts = accounts(&profile.accounts, &format!("{field}.accounts"), check);
    }
}

fn known(names: &[String], name: &str, field: &str, check: &mut Checker) {
    if !names.iter().any(|known| known == name) {
        check.add(
            field,
            format!("profile \"{name}\" does not exist ({})", names.join(", ")),
        );
    }
}

fn texts(list: Option<Vec<String>>) -> Option<Vec<String>> {
    let kept: Vec<String> = list?.iter().filter_map(|text| trimmed(text)).collect();
    (!kept.is_empty()).then_some(kept)
}

fn rules(rules: &mut [Rule], names: &[String], check: &mut Checker) {
    let mut ids = HashSet::new();
    for (at, rule) in rules.iter_mut().enumerate() {
        let field = format!("rules[{at}]");
        if rule.id.is_some_and(|id| !ids.insert(id)) {
            check.add(&format!("{field}.id"), "is used by another rule");
        }
        let when = &mut rule.when;
        when.labels = texts(when.labels.take());
        when.priorities = texts(when.priorities.take());
        if let Some(files) = when.files_min {
            check.int(&format!("{field}.when.filesMin"), files, 1, None);
        }
        if let Some(chars) = when.plan_min_chars {
            check.int(&format!("{field}.when.planMinChars"), chars, 1, None);
        }
        if when.is_empty() {
            check.add(&format!("{field}.when"), "needs at least one condition");
        }
        tidy(&mut rule.profile);
        known(names, &rule.profile, &format!("{field}.profile"), check);
        rule.accounts = accounts(&rule.accounts, &format!("{field}.accounts"), check);
    }
}

fn ladder(ladder: &mut [String], names: &[String], check: &mut Checker) {
    let mut seen: HashSet<String> = HashSet::new();
    for (at, name) in ladder.iter_mut().enumerate() {
        tidy(name);
        let field = format!("ladder[{at}]");
        known(names, name, &field, check);
        if !seen.insert(name.clone()) {
            check.add(&field, "is on the ladder twice");
        }
    }
}

fn optional_profile(
    profile: &mut Option<String>,
    names: &[String],
    field: &str,
    check: &mut Checker,
) {
    *profile = trimmed_option(profile.as_deref());
    if let Some(name) = profile {
        known(names, name, field, check);
    }
}

/// A folder inside the worktree: `/` separators, no leading `./`, `.` for the root itself.
fn relative_dir(text: &str) -> Result<String, String> {
    let text = text.trim().replace('\\', "/");
    let mut chars = text.chars();
    let drive = matches!((chars.next(), chars.next()), (Some(letter), Some(':')) if letter.is_ascii_alphabetic());
    if text.starts_with('/') || drive {
        return Err(format!(
            "\"{text}\" is absolute, only a folder relative to the worktree is allowed"
        ));
    }
    let parts: Vec<&str> = text
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.contains(&"..") {
        return Err(format!(
            "\"{text}\" leaves the worktree (.. is not allowed)"
        ));
    }
    Ok(if parts.is_empty() {
        String::from(".")
    } else {
        parts.join("/")
    })
}

fn setup_steps(steps: &mut [WorktreeSetupStep], check: &mut Checker) {
    for (at, step) in steps.iter_mut().enumerate() {
        tidy(&mut step.run);
        if step.run.is_empty() {
            check.add(&format!("worktreeSetup[{at}].run"), "is empty");
        }
        match relative_dir(&step.cwd) {
            Ok(cwd) => step.cwd = cwd,
            Err(problem) => check.add(&format!("worktreeSetup[{at}].cwd"), problem),
        }
    }
}

/// An own prompt template may only use known placeholders and must keep the required parts.
fn template(kind: PromptKind, text: &str, check: &mut Checker) {
    let field = format!("prompts.{}", kind.as_str());
    if let Some(problem) = size_problem(text) {
        check.add(&field, problem);
        return;
    }
    let unknown = unknown_placeholders(kind, text);
    if !unknown.is_empty() {
        let names: Vec<String> = unknown
            .iter()
            .map(|name| format!("{{{{{name}}}}}"))
            .collect();
        check.add(
            &field,
            format!("uses unknown placeholders {}", names.join(", ")),
        );
    }
    let missing: Vec<&str> = missing_parts(kind, text)
        .iter()
        .map(|part| part.label)
        .collect();
    if !missing.is_empty() {
        check.add(
            &field,
            format!("lacks required parts: {}", missing.join("; ")),
        );
    }
}

fn window(window: Option<&TimeWindow>, check: &mut Checker) {
    if let Some(window) = window
        && (!is_clock(&window.from) || !is_clock(&window.to))
    {
        check.add(
            "window",
            "needs from and to as HH:MM, such as 18:00 and 07:00",
        );
    }
}

impl DispatcherConfig {
    /// Checks a draft, tidies it and returns every refused field at once.
    pub fn normalized(mut self) -> Result<Self, ConfigError> {
        let mut check = Checker::default();
        check.int("wip", self.wip, 0, None);
        check.int("maxAttempts", self.max_attempts, 1, None);
        check.int("pauseMinutesOnLimit", self.pause_minutes_on_limit, 1, None);
        check.int(
            "worktreeSetupTimeoutMinutes",
            self.worktree_setup_timeout_minutes,
            1,
            None,
        );
        check.int(
            "accountLimitPercent",
            self.account_limit_percent,
            50,
            Some(100),
        );
        check.int(
            "accountReserveFiveHour",
            self.account_reserve_five_hour,
            0,
            Some(50),
        );
        check.int("accountReserveWeek", self.account_reserve_week, 0, Some(50));
        check.int(
            "accountResetGraceMinutes",
            self.account_reset_grace_minutes,
            0,
            Some(300),
        );
        window(self.window.as_ref(), &mut check);
        self.opt_in_label = trimmed_option(self.opt_in_label.as_deref());
        self.opt_out_label = trimmed_option(self.opt_out_label.as_deref());
        self.agent_prefix = trimmed(&self.agent_prefix).unwrap_or_else(|| String::from("worker-"));
        claude(&mut self.claude, self.allow_fable, &mut check);
        profiles(&mut self, &mut check);
        let names: Vec<String> = self
            .profiles
            .iter()
            .map(|found| found.name.clone())
            .collect();
        self.default_profile = trimmed(&self.default_profile).unwrap_or_else(|| {
            if names.iter().any(|name| name == FALLBACK_PROFILE) {
                String::from(FALLBACK_PROFILE)
            } else {
                names.first().cloned().unwrap_or_default()
            }
        });
        known(&names, &self.default_profile, "defaultProfile", &mut check);
        ladder(&mut self.ladder, &names, &mut check);
        rules(&mut self.rules, &names, &mut check);
        self.auto_runs(&names, &mut check);
        self.stale(&mut check);
        setup_steps(&mut self.worktree_setup, &mut check);
        for kind in PromptKind::ALL {
            let tidy = self
                .prompts
                .get(kind)
                .filter(|text| !text.trim().is_empty())
                .map(|text| text.replace("\r\n", "\n"));
            if let Some(text) = &tidy {
                template(kind, text, &mut check);
            }
            self.prompts.set(kind, tidy);
        }
        if check.errors.is_empty() {
            Ok(self)
        } else {
            Err(ConfigError(check.errors))
        }
    }

    fn auto_runs(&mut self, names: &[String], check: &mut Checker) {
        let most = Some(MAX_PARALLEL_AUTO_RUNS);
        check.int("research.wip", self.research.wip, 1, most);
        check.int("research.maxPerDay", self.research.max_per_day, 1, None);
        optional_profile(&mut self.research.profile, names, "research.profile", check);
        check.int("planning.wip", self.planning.wip, 1, most);
        optional_profile(&mut self.planning.profile, names, "planning.profile", check);
        let autopilot = &mut self.autopilot;
        autopilot.except_label = trimmed_option(autopilot.except_label.as_deref());
        check.int(
            "autopilot.maxMergeSessions",
            autopilot.max_merge_sessions,
            1,
            most,
        );
    }

    /// A running worker never counts as orphaned: the threshold stays above the longest timeout.
    fn stale(&mut self, check: &mut Checker) {
        let longest = self
            .profiles
            .iter()
            .map(|profile| profile.timeout_minutes)
            .fold(self.claude.timeout_minutes, i64::max);
        if self.stale_minutes < 1 {
            check.add("staleMinutes", "must be a whole number from 1");
        }
        if self.stale_minutes < longest.saturating_add(10) {
            self.stale_minutes = longest.saturating_add(30);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher_config::{
        AccountStrategy, JoinWorktrees, MergeStrategy, RuleCondition, default_template,
    };
    use uuid::Uuid;

    fn fields(config: DispatcherConfig) -> Vec<String> {
        config
            .normalized()
            .expect_err("the draft is refused")
            .0
            .into_iter()
            .map(|error| error.field)
            .collect()
    }

    fn changed(change: impl FnOnce(&mut DispatcherConfig)) -> DispatcherConfig {
        let mut config = DispatcherConfig::default();
        change(&mut config);
        config
    }

    #[test]
    fn the_default_configuration_is_valid_and_stays_as_it_is() {
        let config = DispatcherConfig::default();
        assert_eq!(config.clone().normalized().unwrap(), config);
        assert!(!config.enabled);
        assert_eq!(config.stale_minutes, 150);
        assert_eq!(config.ladder, ["small", "medium", "large"]);
        assert_eq!(config.default_profile, "medium");
        assert_eq!(config.rules.len(), 4);
        assert_eq!(config.claude.allowed_tools.len(), 16);
        assert_eq!(config.merge_strategy, MergeStrategy::Merge);
        assert_eq!(config.join_worktrees, JoinWorktrees::None);
        assert_eq!(config.account_strategy, AccountStrategy::MostFree);
    }

    #[test]
    fn every_refused_field_is_reported_at_once() {
        let found = fields(changed(|config| {
            config.wip = -1;
            config.max_attempts = 0;
            config.research.wip = 6;
            config.window = Some(TimeWindow {
                from: String::from("25:00"),
                to: String::from("18:00"),
            });
            config.default_profile = String::from("huge");
        }));
        assert_eq!(
            found,
            [
                "wip",
                "maxAttempts",
                "window",
                "defaultProfile",
                "research.wip"
            ]
        );
    }

    #[test]
    fn numbers_are_checked_against_their_ranges() {
        let found = fields(changed(|config| {
            config.pause_minutes_on_limit = 0;
            config.worktree_setup_timeout_minutes = 0;
            config.account_limit_percent = 49;
            config.account_reserve_five_hour = 51;
            config.account_reserve_week = -1;
            config.account_reset_grace_minutes = 301;
            config.planning.wip = 0;
            config.autopilot.max_merge_sessions = 6;
            config.research.max_per_day = 0;
            config.claude.max_turns = 0;
            config.claude.timeout_minutes = 0;
        }));
        assert_eq!(
            found,
            [
                "pauseMinutesOnLimit",
                "worktreeSetupTimeoutMinutes",
                "accountLimitPercent",
                "accountReserveFiveHour",
                "accountReserveWeek",
                "accountResetGraceMinutes",
                "claude.maxTurns",
                "claude.timeoutMinutes",
                "research.maxPerDay",
                "planning.wip",
                "autopilot.maxMergeSessions",
            ]
        );
        let edges = changed(|config| {
            config.wip = 0;
            config.account_limit_percent = 50;
            config.account_reserve_five_hour = 50;
            config.account_reset_grace_minutes = 300;
            config.research.wip = 5;
        });
        assert!(edges.normalized().is_ok());
    }

    #[test]
    fn the_window_needs_two_clock_times() {
        for (from, to) in [
            ("8:00", "18:00"),
            ("08:60", "18:00"),
            ("08:00", "24:00"),
            ("", ""),
        ] {
            let config = changed(|config| {
                config.window = Some(TimeWindow {
                    from: from.to_owned(),
                    to: to.to_owned(),
                });
            });
            assert_eq!(fields(config), ["window"], "{from}-{to}");
        }
        let overnight = changed(|config| {
            config.window = Some(TimeWindow {
                from: String::from("18:00"),
                to: String::from("07:00"),
            });
        });
        assert!(overnight.normalized().is_ok());
    }

    #[test]
    fn models_and_efforts_must_be_known_and_fable_needs_a_release() {
        assert_eq!(model_problem("sonnet", false), None);
        assert_eq!(model_problem("claude-opus-4-5[1m]", false), None);
        assert!(
            model_problem("gpt", false)
                .unwrap()
                .contains("unknown model")
        );
        assert!(model_problem("claude-", false).is_some());
        assert!(model_problem("claude-Opus", false).is_some());
        assert!(
            model_problem("fable", false)
                .unwrap()
                .contains("not allowed")
        );
        assert!(model_problem("best", false).is_some());
        assert!(model_problem("claude-fable-1", false).is_some());
        assert_eq!(model_problem("fable", true), None);
        assert_eq!(effort_problem("xhigh"), None);
        assert!(effort_problem("extreme").unwrap().contains("low, medium"));
        let refused = fields(changed(|config| {
            config.claude.model = Some(String::from("fable"));
            config.claude.effort = Some(String::from("huge"));
            config.profiles[0].model = Some(String::from("nope"));
            config.profiles[1].effort = Some(String::from("nope"));
        }));
        assert_eq!(
            refused,
            [
                "claude.model",
                "claude.effort",
                "profiles[0].model",
                "profiles[1].effort"
            ]
        );
        let allowed = changed(|config| {
            config.allow_fable = true;
            config.profiles[0].model = Some(String::from("fable[1m]"));
        });
        assert!(allowed.normalized().is_ok());
    }

    #[test]
    fn without_profiles_one_is_made_from_the_claude_defaults() {
        let config = changed(|config| {
            config.profiles.clear();
            config.default_profile = String::new();
            config.ladder.clear();
            config.rules.clear();
        });
        let config = config.normalized().unwrap();
        assert_eq!(config.profiles.len(), 1);
        assert_eq!(config.profiles[0].name, "medium");
        assert_eq!(config.profiles[0].model.as_deref(), Some("sonnet"));
        assert_eq!(config.profiles[0].max_turns, 80);
        assert_eq!(config.default_profile, "medium");
    }

    #[test]
    fn a_blank_default_profile_prefers_medium_else_the_first() {
        let config = changed(|config| {
            config.default_profile = String::from("  ");
            config.profiles.retain(|profile| profile.name != "medium");
            config.ladder = vec![String::from("small")];
            config.rules.truncate(1);
        });
        assert_eq!(config.normalized().unwrap().default_profile, "small");
        let config = changed(|config| config.default_profile = String::new());
        assert_eq!(config.normalized().unwrap().default_profile, "medium");
    }

    #[test]
    fn profile_names_are_trimmed_unique_and_not_empty() {
        let config = changed(|config| config.profiles[1].name = String::from("  small "));
        assert_eq!(
            fields(config),
            ["profiles[1].name", "defaultProfile", "ladder[1]"]
        );
        let config = changed(|config| config.profiles[0].name = String::from(" "));
        assert!(fields(config).contains(&String::from("profiles[0].name")));
        let config = changed(|config| config.profiles[0].name = "x".repeat(61));
        assert!(fields(config).contains(&String::from("profiles[0].name")));
        let config = changed(|config| {
            config.profiles[2].name = String::from("  big ");
            config.ladder[2] = String::from("big");
            config.rules[1].profile = String::from("big");
            config.rules[2].profile = String::from(" big");
            config.rules[3].profile = String::from("big");
        });
        let config = config.normalized().unwrap();
        assert_eq!(config.profiles[2].name, "big");
        assert_eq!(config.rules[2].profile, "big");
    }

    #[test]
    fn profile_limits_must_be_positive() {
        let config = changed(|config| {
            config.profiles[0].max_turns = 0;
            config.profiles[2].timeout_minutes = -5;
        });
        assert_eq!(
            fields(config),
            ["profiles[0].maxTurns", "profiles[2].timeoutMinutes"]
        );
    }

    #[test]
    fn accounts_are_trimmed_deduplicated_and_must_look_like_ids() {
        let config = changed(|config| {
            config.profiles[2].accounts = vec![
                String::from(" max "),
                String::from("work"),
                String::from("max"),
            ];
            config.rules[0].accounts = vec![String::from("Max Private")];
        });
        assert_eq!(fields(config), ["rules[0].accounts[0]"]);
        let config = changed(|config| {
            config.profiles[2].accounts = vec![
                String::from(" max "),
                String::from("work"),
                String::from("max"),
            ];
        });
        let config = config.normalized().unwrap();
        assert_eq!(config.profiles[2].accounts, ["max", "work"]);
        let references = config.account_references();
        assert_eq!(
            references,
            [
                (String::from("profiles[2].accounts[0]"), "max"),
                (String::from("profiles[2].accounts[1]"), "work"),
            ]
        );
    }

    #[test]
    fn rules_need_a_condition_and_a_known_profile() {
        let config = changed(|config| {
            config.rules[0].when = RuleCondition::default();
            config.rules[1].profile = String::from("giant");
            config.rules[2].when.files_min = Some(0);
            config.rules[3].when.plan_min_chars = Some(0);
        });
        assert_eq!(
            fields(config),
            [
                "rules[0].when",
                "rules[1].profile",
                "rules[2].when.filesMin",
                "rules[3].when.planMinChars"
            ]
        );
    }

    #[test]
    fn blank_labels_do_not_count_as_a_condition() {
        let config = changed(|config| {
            config.rules[0].when.labels = Some(vec![String::from(" "), String::new()]);
        });
        assert_eq!(fields(config), ["rules[0].when"]);
        let config = changed(|config| {
            config.rules[0].when.labels = Some(vec![
                String::from(" docs "),
                String::new(),
                String::from("typo"),
            ]);
        });
        let config = config.normalized().unwrap();
        assert_eq!(
            config.rules[0].when.labels.as_deref(),
            Some(&[String::from("docs"), String::from("typo")][..])
        );
    }

    #[test]
    fn the_ladder_names_known_profiles_once() {
        let config = changed(|config| {
            config.ladder = vec![
                String::from("small"),
                String::from("ghost"),
                String::from("small"),
            ];
        });
        assert_eq!(fields(config), ["ladder[1]", "ladder[2]"]);
        let config = changed(|config| config.ladder.clear());
        assert_eq!(config.normalized().unwrap().ladder, Vec::<String>::new());
    }

    #[test]
    fn research_and_planning_profiles_must_exist() {
        let config = changed(|config| {
            config.research.profile = Some(String::from("ghost"));
            config.planning.profile = Some(String::from(" large "));
        });
        assert_eq!(fields(config.clone()), ["research.profile"]);
        let mut fixed = config;
        fixed.research.profile = Some(String::from("  "));
        let fixed = fixed.normalized().unwrap();
        assert_eq!(fixed.research.profile, None);
        assert_eq!(fixed.planning.profile.as_deref(), Some("large"));
    }

    #[test]
    fn the_stale_threshold_stays_above_the_longest_timeout() {
        let config = DispatcherConfig::default().normalized().unwrap();
        assert_eq!(config.stale_minutes, 150);
        let config = changed(|config| config.stale_minutes = 129)
            .normalized()
            .unwrap();
        assert_eq!(config.stale_minutes, 150);
        let config = changed(|config| config.stale_minutes = 130)
            .normalized()
            .unwrap();
        assert_eq!(config.stale_minutes, 130);
        let config = changed(|config| config.profiles[0].timeout_minutes = 300)
            .normalized()
            .unwrap();
        assert_eq!(config.stale_minutes, 330);
        assert_eq!(
            fields(changed(|config| config.stale_minutes = -3)),
            ["staleMinutes"]
        );
        assert_eq!(
            fields(changed(|config| config.stale_minutes = 0)),
            ["staleMinutes"]
        );
    }

    #[test]
    fn row_ids_are_not_shared() {
        let id = Uuid::from_u128(7);
        let found = fields(changed(|config| {
            config.profiles[0].id = Some(id);
            config.profiles[1].id = Some(id);
            config.rules[0].id = Some(id);
            config.rules[1].id = Some(id);
        }));
        assert_eq!(found, ["profiles[1].id", "rules[1].id"]);
    }

    #[test]
    fn setup_steps_stay_inside_the_worktree() {
        assert_eq!(relative_dir(" ./web//app/ "), Ok(String::from("web/app")));
        assert_eq!(relative_dir(""), Ok(String::from(".")));
        assert_eq!(relative_dir(".\\a\\b"), Ok(String::from("a/b")));
        assert!(relative_dir("/etc").unwrap_err().contains("absolute"));
        assert!(relative_dir("C:\\x").unwrap_err().contains("absolute"));
        assert!(relative_dir("a/../b").unwrap_err().contains("leaves"));
        let config = changed(|config| {
            config.worktree_setup = vec![
                WorktreeSetupStep {
                    cwd: String::from("/abs"),
                    run: String::from("npm ci"),
                },
                WorktreeSetupStep {
                    cwd: String::from("web"),
                    run: String::from("  "),
                },
                WorktreeSetupStep {
                    cwd: String::new(),
                    run: String::from(" npm ci "),
                },
            ];
        });
        assert_eq!(
            fields(config),
            ["worktreeSetup[0].cwd", "worktreeSetup[1].run"]
        );
    }

    #[test]
    fn blank_text_settings_fall_back_or_clear() {
        let config = changed(|config| {
            config.agent_prefix = String::from(" ");
            config.opt_in_label = Some(String::from("  "));
            config.opt_out_label = Some(String::from(" skip "));
            config.claude.command = String::new();
            config.claude.permission_mode = Some(String::from(" "));
            config.claude.git_bash_path = Some(String::from(" C:/git/bash.exe "));
            config.claude.allowed_tools =
                vec![String::from(" Read "), String::new(), String::from("Edit")];
            config.autopilot.except_label = Some(String::new());
        });
        let config = config.normalized().unwrap();
        assert_eq!(config.agent_prefix, "worker-");
        assert_eq!(config.opt_in_label, None);
        assert_eq!(config.opt_out_label.as_deref(), Some("skip"));
        assert_eq!(config.claude.command, "claude");
        assert_eq!(config.claude.permission_mode, None);
        assert_eq!(
            config.claude.git_bash_path.as_deref(),
            Some("C:/git/bash.exe")
        );
        assert_eq!(config.claude.allowed_tools, ["Read", "Edit"]);
        assert_eq!(config.autopilot.except_label, None);
    }

    #[test]
    fn prompt_templates_keep_their_text_but_lose_blank_ones_and_carriage_returns() {
        let own = format!("  {}", default_template(PromptKind::Work));
        let config = changed(|config| {
            config.prompts.work = Some(own.replace('\n', "\r\n"));
            config.prompts.plan = Some(String::from("   "));
        });
        let config = config.normalized().unwrap();
        assert_eq!(config.prompts.work.as_deref(), Some(own.as_str()));
        assert_eq!(config.prompts.plan, None);
        assert_eq!(config.prompts.entries().len(), 1);
    }

    #[test]
    fn a_huge_prompt_template_is_refused_per_field_without_being_checked() {
        let huge = "--author ".repeat(2 * 1024 * 1024 / 9);
        let started = std::time::Instant::now();
        let errors = changed(|config| config.prompts.work = Some(huge.clone()))
            .normalized()
            .unwrap_err()
            .0;
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, "prompts.work");
        assert_eq!(errors[0].message, "is longer than 64 KiB");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn prompt_templates_with_unknown_placeholders_or_missing_parts_are_refused_per_field() {
        let error = changed(|config| {
            config.prompts.work = Some(String::from("Do {{ticketId}} as {{agent}} {{nope}}"));
            config.prompts.research = Some(String::from("Research {{questionId}} as {{agent}}"));
        })
        .normalized()
        .unwrap_err();
        let fields: Vec<(&str, &str)> = error
            .0
            .iter()
            .map(|found| (found.field.as_str(), found.message.as_str()))
            .collect();
        assert_eq!(
            fields,
            [
                ("prompts.work", "uses unknown placeholders {{nope}}"),
                (
                    "prompts.work",
                    "lacks required parts: Outcome done: nullradix ticket done {{ticketId}}; Outcome blocked: nullradix ticket set {{ticketId}} --status blocked; Outcome question: nullradix question add ... --ticket {{ticketId}}; Attribution --author {{agent}}"
                ),
                (
                    "prompts.research",
                    "lacks required parts: Saving: nullradix research submit {{questionId}}; File research.json"
                ),
            ]
        );
    }

    #[test]
    fn every_default_template_is_accepted_as_an_own_one() {
        let config = changed(|config| {
            for kind in PromptKind::ALL {
                config
                    .prompts
                    .set(kind, Some(default_template(kind).to_owned()));
            }
        });
        assert!(config.normalized().is_ok());
    }

    #[test]
    fn field_errors_read_as_a_sentence() {
        let error = changed(|config| config.wip = -1).normalized().unwrap_err();
        assert_eq!(
            error.to_string(),
            "invalid dispatcher configuration: wip: must be a whole number from 0"
        );
    }
}
