//! Builds the `claude -p` call of a run from the configuration; the prompt goes over stdin.

use std::time::Duration;

use nullradix_domain::{ClaudeSettings, Profile, RunKind};

use crate::hooks::{SESSION_VAR, TICKET_VAR};
use crate::runs::RunEnv;

/// Also allowed in a merge session.
pub const MERGE_SESSION_TOOLS: [&str; 2] = ["Bash(git merge:*)", "Bash(git checkout:*)"];

/// Permission mode of research and plan runs: only the listed tools pass, as `-p` cannot ask.
pub const RESTRICTED_PERMISSION_MODE: &str = "default";

/// The only tools of a research run; there is no `git grep`, as `-O` starts a program.
pub const RESEARCH_TOOLS: [&str; 12] = [
    "Read",
    "Glob",
    "Grep",
    "Write(./research.json)",
    "Edit(./research.json)",
    "Bash(nullradix question show:*)",
    "Bash(nullradix ticket show:*)",
    "Bash(nullradix ticket list:*)",
    "Bash(nullradix research submit:*)",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(git diff:*)",
];

/// Denials beat every permission, including the user's own settings.
pub const RESEARCH_DENIED_TOOLS: [&str; 14] = [
    "MultiEdit",
    "NotebookEdit",
    "Bash(git commit:*)",
    "Bash(git push:*)",
    "Bash(git checkout:*)",
    "Bash(git reset:*)",
    "Bash(git switch:*)",
    "Bash(git merge:*)",
    "Bash(git rebase:*)",
    "Bash(npm:*)",
    "Bash(node:*)",
    "Bash(git diff --output:*)",
    "Bash(git log --output:*)",
    "Bash(git show --output:*)",
];

/// The only tools of a plan run.
pub const PLANNING_TOOLS: [&str; 13] = [
    "Read",
    "Glob",
    "Grep",
    "Write(./plan.json)",
    "Edit(./plan.json)",
    "Bash(nullradix ticket show:*)",
    "Bash(nullradix ticket list:*)",
    "Bash(nullradix plan write:*)",
    "Bash(nullradix comment:*)",
    "Bash(nullradix note list:*)",
    "Bash(nullradix question add:*)",
    "Bash(git log:*)",
    "Bash(git show:*)",
];

/// Denied in plan runs on top of the research denials; `plan write` changes the ticket.
pub const PLANNING_ONLY_DENIED_TOOLS: [&str; 5] = [
    "Bash(nullradix ticket set:*)",
    "Bash(nullradix ticket claim:*)",
    "Bash(nullradix ticket start:*)",
    "Bash(nullradix ticket done:*)",
    "Bash(nullradix ticket delete:*)",
];

/// Claude Code backgrounds long Bash calls and `-p` then ends early, so raise the limits.
const BASH_DEFAULT_TIMEOUT_MS: &str = "900000";
const BASH_MAX_TIMEOUT_MS: &str = "1800000";

/// Everything one run's call depends on.
#[derive(Debug, Clone, Copy)]
pub struct InvocationInput<'a> {
    pub kind: RunKind,
    pub claude: &'a ClaudeSettings,
    pub profile: &'a Profile,
    /// Allowed in addition to the kind's own tools.
    pub extra_tools: &'a [String],
    /// Ticket key for the ticket variable; research runs have none.
    pub ticket_key: Option<&'a str>,
    pub run_id: &'a str,
    /// Folder of the chosen account; none keeps Claude's own login.
    pub config_dir: Option<&'a str>,
    /// Git Bash on Windows; the runner looks it up when none is configured.
    pub git_bash: Option<&'a str>,
}

/// The program, its arguments and the environment changes of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub env: RunEnv,
    pub timeout: Duration,
}

/// The model of a run: the profile's, else the general one.
#[must_use]
pub fn effective_model(claude: &ClaudeSettings, profile: &Profile) -> Option<String> {
    profile.model.clone().or_else(|| claude.model.clone())
}

/// The effort of a run; Haiku knows none.
#[must_use]
pub fn effective_effort(claude: &ClaudeSettings, profile: &Profile) -> Option<String> {
    let haiku = effective_model(claude, profile)
        .is_some_and(|model| model.starts_with("haiku") || model.contains("-haiku"));
    if haiku {
        return None;
    }
    profile.effort.clone().or_else(|| claude.effort.clone())
}

fn strings(tools: &[&str]) -> Vec<String> {
    tools.iter().map(|tool| (*tool).to_owned()).collect()
}

fn denied_tools(kind: RunKind) -> Vec<String> {
    let mut denied = strings(&RESEARCH_DENIED_TOOLS);
    if kind == RunKind::Plan {
        denied.extend(strings(&PLANNING_ONLY_DENIED_TOOLS));
    }
    denied
}

fn allowed_tools(input: &InvocationInput<'_>) -> Vec<String> {
    let mut tools = match input.kind {
        RunKind::Research => strings(&RESEARCH_TOOLS),
        RunKind::Plan => strings(&PLANNING_TOOLS),
        RunKind::Work => input.claude.allowed_tools.clone(),
        RunKind::Merge => {
            let mut tools = input.claude.allowed_tools.clone();
            tools.extend(strings(&MERGE_SESSION_TOOLS));
            tools
        }
    };
    for tool in input.extra_tools {
        if !tools.contains(tool) {
            tools.push(tool.clone());
        }
    }
    tools
}

/// Both tool lists take every value up to the next option, so they come last.
fn args(input: &InvocationInput<'_>) -> Vec<String> {
    let claude = input.claude;
    let restricted = input.kind.is_restricted();
    let mut args = vec![
        String::from("-p"),
        String::from("--output-format"),
        String::from("stream-json"),
        String::from("--verbose"),
        String::from("--max-turns"),
        input.profile.max_turns.to_string(),
    ];
    if let Some(model) = effective_model(claude, input.profile) {
        args.extend([String::from("--model"), model]);
    }
    if let Some(effort) = effective_effort(claude, input.profile) {
        args.extend([String::from("--effort"), effort]);
    }
    let mode = if restricted {
        Some(RESTRICTED_PERMISSION_MODE.to_owned())
    } else {
        claude.permission_mode.clone()
    };
    if let Some(mode) = mode {
        args.extend([String::from("--permission-mode"), mode]);
    }
    if restricted {
        args.push(String::from("--disallowedTools"));
        args.extend(denied_tools(input.kind));
    } else {
        args.extend(
            claude
                .extra_args
                .iter()
                .filter(|arg| *arg != "--verbose")
                .cloned(),
        );
    }
    let tools = allowed_tools(input);
    if !tools.is_empty() {
        args.push(String::from("--allowedTools"));
        args.extend(tools);
    }
    args
}

fn env(input: &InvocationInput<'_>) -> RunEnv {
    let mut env = RunEnv::default();
    if let Some(dir) = input.config_dir.filter(|dir| !dir.is_empty()) {
        env.set
            .insert(String::from("CLAUDE_CONFIG_DIR"), dir.to_owned());
        // a token in the daemon env would override the login of the folder
        env.remove.push(String::from("CLAUDE_CODE_OAUTH_TOKEN"));
    }
    if input.claude.use_subscription {
        env.remove.push(String::from("ANTHROPIC_API_KEY"));
        env.remove.push(String::from("ANTHROPIC_AUTH_TOKEN"));
    }
    // the daemon may run inside a session; the run is a session of its own
    env.remove.push(SESSION_VAR.to_owned());
    env.remove.push(String::from("CLAUDECODE"));
    env.remove.push(String::from("CLAUDE_CODE_ENTRYPOINT"));
    match input.ticket_key {
        Some(key) if input.kind != RunKind::Research => {
            env.set.insert(TICKET_VAR.to_owned(), key.to_owned());
        }
        _ => env.remove.push(TICKET_VAR.to_owned()),
    }
    env.set
        .insert(String::from("NULLRADIX_RUN"), input.run_id.to_owned());
    env.defaults.insert(
        String::from("BASH_DEFAULT_TIMEOUT_MS"),
        BASH_DEFAULT_TIMEOUT_MS.to_owned(),
    );
    env.defaults.insert(
        String::from("BASH_MAX_TIMEOUT_MS"),
        BASH_MAX_TIMEOUT_MS.to_owned(),
    );
    // without these Claude Code uses the PowerShell tool, which the Bash rules do not cover
    if let Some(path) = input.git_bash {
        env.set.insert(
            String::from("CLAUDE_CODE_USE_POWERSHELL_TOOL"),
            String::from("0"),
        );
        env.set
            .insert(String::from("CLAUDE_CODE_GIT_BASH_PATH"), path.to_owned());
    }
    env
}

/// The call of a run; pure, so every rule has a test.
#[must_use]
pub fn build(input: &InvocationInput<'_>) -> Invocation {
    Invocation {
        program: input.claude.command.clone(),
        args: args(input),
        env: env(input),
        timeout: Duration::from_secs(
            u64::try_from(input.profile.timeout_minutes).unwrap_or(0) * 60,
        ),
    }
}

#[cfg(test)]
mod tests {
    use nullradix_domain::DispatcherConfig;

    use super::*;

    fn input<'a>(
        claude: &'a ClaudeSettings,
        profile: &'a Profile,
        kind: RunKind,
        ticket_key: Option<&'a str>,
        config_dir: Option<&'a str>,
        git_bash: Option<&'a str>,
    ) -> InvocationInput<'a> {
        InvocationInput {
            kind,
            claude,
            profile,
            extra_tools: &[],
            ticket_key,
            run_id: "run-1",
            config_dir,
            git_bash,
        }
    }

    fn config() -> DispatcherConfig {
        let mut config = DispatcherConfig::default();
        config.claude.allowed_tools = vec![String::from("Read"), String::from("Bash(nullradix:*)")];
        config.claude.extra_args = vec![String::from("--verbose"), String::from("--add-dir=x")];
        config
    }

    fn call(kind: RunKind, config: &DispatcherConfig, extra: &[String]) -> Invocation {
        let profile = config.profile("medium").unwrap();
        build(&InvocationInput {
            kind,
            claude: &config.claude,
            profile,
            extra_tools: extra,
            ticket_key: Some("ARC-0001"),
            run_id: "run-1",
            config_dir: None,
            git_bash: None,
        })
    }

    fn position(args: &[String], value: &str) -> usize {
        args.iter().position(|arg| arg == value).unwrap()
    }

    #[test]
    fn a_work_run_starts_with_the_fixed_flags_and_ends_with_the_allowed_tools() {
        let call = call(RunKind::Work, &config(), &[]);
        assert_eq!(call.program, "claude");
        assert_eq!(
            call.args[..6],
            [
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--max-turns",
                "80"
            ]
        );
        assert_eq!(call.args[position(&call.args, "--model") + 1], "opus");
        assert_eq!(call.args[position(&call.args, "--effort") + 1], "high");
        assert_eq!(
            call.args[position(&call.args, "--permission-mode") + 1],
            "acceptEdits"
        );
        let tools = position(&call.args, "--allowedTools");
        assert_eq!(
            call.args[tools..],
            ["--allowedTools", "Read", "Bash(nullradix:*)"]
        );
        assert!(!call.args.contains(&String::from("--disallowedTools")));
        assert_eq!(call.timeout, Duration::from_mins(90));
    }

    #[test]
    fn extra_arguments_lose_a_second_verbose_and_come_before_the_tools() {
        let call = call(RunKind::Work, &config(), &[]);
        assert_eq!(
            call.args.iter().filter(|arg| *arg == "--verbose").count(),
            1
        );
        assert!(position(&call.args, "--add-dir=x") < position(&call.args, "--allowedTools"));
    }

    #[test]
    fn a_merge_session_may_also_merge_and_extra_tools_are_not_repeated() {
        let extra = vec![String::from("Read"), String::from("Bash(make:*)")];
        let call = call(RunKind::Merge, &config(), &extra);
        let tools = &call.args[position(&call.args, "--allowedTools") + 1..];
        assert_eq!(
            tools,
            [
                "Read",
                "Bash(nullradix:*)",
                "Bash(git merge:*)",
                "Bash(git checkout:*)",
                "Bash(make:*)"
            ]
        );
    }

    #[test]
    fn a_research_run_may_submit_its_research_and_diff_but_not_write_with_it() {
        let call = call(RunKind::Research, &config(), &[]);
        let allowed = position(&call.args, "--allowedTools");
        let end = call.args[allowed + 1..]
            .iter()
            .position(|arg| arg.starts_with("--"))
            .map_or(call.args.len(), |at| allowed + 1 + at);
        let tools = &call.args[allowed + 1..end];
        for tool in ["Bash(nullradix research submit:*)", "Bash(git diff:*)"] {
            assert!(tools.iter().any(|allowed| allowed == tool), "{tool}");
        }
        assert!(RESEARCH_DENIED_TOOLS.contains(&"Bash(git diff --output:*)"));
    }

    #[test]
    fn research_and_plan_runs_ignore_the_general_settings() {
        let mut risky = config();
        risky.claude.permission_mode = Some(String::from("bypassPermissions"));
        risky.claude.extra_args = vec![String::from("--dangerously-skip-permissions")];
        risky.claude.allowed_tools = vec![String::from("Bash(*)")];
        for kind in [RunKind::Research, RunKind::Plan] {
            let call = call(kind, &risky, &[]);
            assert!(
                !call
                    .args
                    .contains(&String::from("--dangerously-skip-permissions"))
            );
            assert!(!call.args.contains(&String::from("Bash(*)")));
            assert_eq!(
                call.args[position(&call.args, "--permission-mode") + 1],
                "default"
            );
            assert!(
                position(&call.args, "--disallowedTools") < position(&call.args, "--allowedTools")
            );
        }
        let research = call(RunKind::Research, &risky, &[]);
        let plan = call(RunKind::Plan, &risky, &[]);
        assert!(
            research
                .args
                .contains(&String::from("Write(./research.json)"))
        );
        assert!(plan.args.contains(&String::from("Write(./plan.json)")));
        assert!(
            !research
                .args
                .contains(&String::from("Bash(nullradix ticket done:*)"))
        );
        assert!(
            plan.args
                .contains(&String::from("Bash(nullradix ticket done:*)"))
        );
    }

    #[test]
    fn the_profile_model_wins_over_the_general_one() {
        let mut config = config();
        config.claude.model = Some(String::from("haiku"));
        let mut profile = config.profile("medium").unwrap().clone();
        profile.model = None;
        profile.effort = None;
        assert_eq!(
            effective_model(&config.claude, &profile).as_deref(),
            Some("haiku")
        );
        assert_eq!(effective_effort(&config.claude, &profile), None);
        profile.model = Some(String::from("opus"));
        assert_eq!(
            effective_model(&config.claude, &profile).as_deref(),
            Some("opus")
        );
    }

    #[test]
    fn haiku_gets_no_effort_even_when_the_profile_names_one() {
        let mut config = config();
        let mut profile = config.profile("medium").unwrap().clone();
        profile.effort = Some(String::from("high"));
        profile.model = Some(String::from("opus"));
        assert_eq!(
            effective_effort(&config.claude, &profile).as_deref(),
            Some("high")
        );
        for model in ["haiku", "claude-haiku-4-5"] {
            profile.model = Some(String::from(model));
            assert_eq!(effective_effort(&config.claude, &profile), None, "{model}");
            let args = build(&input(
                &config.claude,
                &profile,
                RunKind::Work,
                Some("ARC-0001"),
                None,
                None,
            ))
            .args;
            assert!(args.contains(&String::from(model)), "{args:?}");
            assert!(!args.contains(&String::from("--effort")), "{args:?}");
        }
        // the general model counts when the profile names none
        profile.model = None;
        config.claude.model = Some(String::from("haiku"));
        assert_eq!(effective_effort(&config.claude, &profile), None);
    }

    #[test]
    fn the_environment_follows_the_subscription_account_and_ticket() {
        let mut config = config();
        let profile = config.profile("medium").unwrap().clone();
        let env = build(&input(
            &config.claude,
            &profile,
            RunKind::Work,
            Some("ARC-0001"),
            Some("/accounts/a"),
            Some("C:/Git/bin/bash.exe"),
        ))
        .env;
        assert_eq!(env.set["NULLRADIX_TICKET"], "ARC-0001");
        assert_eq!(env.set["NULLRADIX_RUN"], "run-1");
        assert_eq!(env.set["CLAUDE_CONFIG_DIR"], "/accounts/a");
        assert!(
            env.remove
                .contains(&String::from("CLAUDE_CODE_OAUTH_TOKEN"))
        );
        assert_eq!(env.set["CLAUDE_CODE_USE_POWERSHELL_TOOL"], "0");
        assert_eq!(env.set["CLAUDE_CODE_GIT_BASH_PATH"], "C:/Git/bin/bash.exe");
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "NULLRADIX_SESSION",
            "CLAUDECODE",
        ] {
            assert!(env.remove.contains(&String::from(name)), "{name}");
        }
        assert_eq!(env.defaults["BASH_DEFAULT_TIMEOUT_MS"], "900000");
        assert_eq!(env.defaults["BASH_MAX_TIMEOUT_MS"], "1800000");

        let research = build(&input(
            &config.claude,
            &profile,
            RunKind::Research,
            Some("ARC-0001"),
            None,
            None,
        ))
        .env;
        assert!(!research.set.contains_key("NULLRADIX_TICKET"));
        assert!(research.remove.contains(&String::from("NULLRADIX_TICKET")));
        assert!(!research.set.contains_key("CLAUDE_CONFIG_DIR"));
        assert!(
            !research
                .remove
                .contains(&String::from("CLAUDE_CODE_OAUTH_TOKEN"))
        );
        assert!(!research.set.contains_key("CLAUDE_CODE_GIT_BASH_PATH"));

        config.claude.use_subscription = false;
        let api_key = build(&input(
            &config.claude,
            &profile,
            RunKind::Work,
            None,
            None,
            None,
        ))
        .env;
        assert!(!api_key.remove.contains(&String::from("ANTHROPIC_API_KEY")));
    }
}
