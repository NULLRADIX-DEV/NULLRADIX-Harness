use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use nullradix_api::{DispatcherConfigDto, DispatcherConfigView, UpdateDispatcherConfig};
use nullradix_client::Client;
use serde_json::Value;

#[derive(Subcommand)]
pub(crate) enum DispatcherCommand {
    /// Show what the dispatcher does and which planned tickets are ready
    Status,
    /// Run one pass now
    Run,
    /// Stop starting new tickets for a while
    Pause {
        /// Minutes to pause, 1 to 1440
        #[arg(long, default_value_t = 60)]
        minutes: u32,
    },
    /// Lift the pause
    Resume,
    /// Start new tickets, whatever the configuration says
    Enable,
    /// Stop starting new tickets, whatever the configuration says
    Disable,
    /// Show and change the dispatcher configuration
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Show, set, reset and preview the prompt templates of runs
    #[command(subcommand)]
    Prompt(crate::dispatcher_prompt::PromptCommand),
}

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    /// Show the configuration in force
    Show {
        /// Print JSON, the format `set --file` reads
        #[arg(long)]
        json: bool,
    },
    /// Change the configuration from a JSON file and a few switches
    Set(SetArgs),
}

#[derive(Args)]
pub(crate) struct SetArgs {
    /// JSON with the fields to change, or `-` for standard input; other fields stay
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
    /// Start new runs, or not (true or false)
    #[arg(long, value_name = "BOOL")]
    enabled: Option<bool>,
    /// Tickets worked on at the same time
    #[arg(long, value_name = "COUNT")]
    wip: Option<i64>,
}

pub(crate) fn run(command: DispatcherCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        DispatcherCommand::Status => print!("{}", describe_status(&client.dispatcher_status()?)),
        DispatcherCommand::Run => print!("{}", describe_pass(&client.run_dispatcher_pass()?)),
        DispatcherCommand::Pause { minutes } => {
            let status = client.pause_dispatcher(&nullradix_api::PauseDispatcher {
                minutes: Some(minutes),
                until: None,
            })?;
            print!("{}", describe_status(&status));
        }
        DispatcherCommand::Resume => print!("{}", describe_status(&client.resume_dispatcher()?)),
        DispatcherCommand::Enable => {
            print!("{}", describe_status(&client.set_dispatcher_enabled(true)?));
        }
        DispatcherCommand::Disable => {
            print!(
                "{}",
                describe_status(&client.set_dispatcher_enabled(false)?)
            );
        }
        DispatcherCommand::Config(ConfigCommand::Show { json }) => {
            let view = client.dispatcher_config()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&view.config)?);
            } else {
                print!("{}", describe(&view));
            }
        }
        DispatcherCommand::Config(ConfigCommand::Set(args)) => set(client, &args)?,
        DispatcherCommand::Prompt(command) => {
            return crate::dispatcher_prompt::run(command, client);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn set(client: &Client, args: &SetArgs) -> anyhow::Result<()> {
    if args.file.is_none() && args.enabled.is_none() && args.wip.is_none() {
        bail!("nothing to change, give --file, --enabled or --wip");
    }
    let view = client.dispatcher_config()?;
    let mut config = serde_json::to_value(&view.config)?;
    if let Some(path) = &args.file {
        merge(&mut config, read_json(path)?);
    }
    if let Some(enabled) = args.enabled {
        config["enabled"] = Value::Bool(enabled);
    }
    if let Some(wip) = args.wip {
        config["wip"] = Value::from(wip);
    }
    let config: DispatcherConfigDto =
        serde_json::from_value(config).context("the configuration does not fit")?;
    let saved = client.update_dispatcher_config(&UpdateDispatcherConfig {
        version: view.version,
        config,
    })?;
    if saved.version == view.version {
        println!("nothing changed, still version {}", saved.version);
    } else {
        println!(
            "saved dispatcher configuration, version {} ({})",
            saved.version, saved.hash
        );
    }
    Ok(())
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    let text = if path == Path::new("-") {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .context("cannot read standard input")?;
        text
    } else {
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?
    };
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

/// Objects merge key by key, anything else replaces; `null` sets a field to none.
fn merge(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                merge(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target, patch) => *target = patch,
    }
}

fn describe_pass(report: &nullradix_api::PassReportDto) -> String {
    let mut out = String::new();
    if let Some(idle) = &report.idle {
        let _ = writeln!(out, "idle: {idle}");
    }
    for (title, keys) in [
        ("started", &report.started),
        ("waiting for a place", &report.queued),
        ("given back", &report.released),
    ] {
        if !keys.is_empty() {
            let _ = writeln!(out, "{title}: {}", keys.join(", "));
        }
    }
    for error in &report.errors {
        let _ = writeln!(out, "error: {error}");
    }
    if out.is_empty() {
        out.push_str(
            "nothing to do
",
        );
    }
    out
}

fn describe_status(status: &nullradix_api::DispatcherStatusDto) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}: {}", status.state_word(), status.message);
    let _ = writeln!(
        out,
        "{} of {} places in use, {} planning",
        status.active, status.wip, status.planning
    );
    for run in &status.runs {
        let _ = writeln!(
            out,
            "  running {} {} [{}]",
            shown(run.subject.as_deref()),
            run.id,
            run.profile
        );
    }
    for ticket in &status.tickets {
        let _ = writeln!(out, "  {} {}: {}", ticket.key, ticket.title, ticket.reason);
    }
    for error in &status.last_errors {
        let _ = writeln!(out, "  error: {error}");
    }
    out
}

fn shown(text: Option<&str>) -> &str {
    text.unwrap_or("-")
}

fn describe(view: &DispatcherConfigView) -> String {
    let config = &view.config;
    let mut out = String::new();
    let state = if config.enabled { "on" } else { "off" };
    let _ = writeln!(
        out,
        "dispatcher {state}, version {} ({})",
        view.version, view.hash
    );
    let window = config.window.as_ref().map_or_else(
        || String::from("always"),
        |window| format!("{}-{}", window.from, window.to),
    );
    let _ = writeln!(
        out,
        "window {window}, wip {}, attempts {}",
        config.wip, config.max_attempts
    );
    let _ = writeln!(
        out,
        "labels in {} out {}, pause {} min on limit",
        shown(config.opt_in_label.as_deref()),
        shown(config.opt_out_label.as_deref()),
        config.pause_minutes_on_limit
    );
    let _ = writeln!(
        out,
        "merge {}, review before merge {}",
        format!("{:?}", config.merge_strategy).to_lowercase(),
        config.review_before_merge
    );
    let _ = writeln!(out, "profiles (default {}):", config.default_profile);
    for profile in &config.profiles {
        let _ = writeln!(
            out,
            "  {:<10} {:<8} {:<6} {:>4} turns {:>4} min  accounts {}",
            profile.name,
            shown(profile.model.as_deref()),
            shown(profile.effort.as_deref()),
            profile.max_turns,
            profile.timeout_minutes,
            if profile.accounts.is_empty() {
                String::from("any")
            } else {
                profile.accounts.join(",")
            }
        );
    }
    let ladder = if config.escalate {
        config.ladder.join(" > ")
    } else {
        String::from("off")
    };
    let _ = writeln!(out, "ladder {ladder}");
    let _ = writeln!(out, "rules: {}", config.rules.len());
    for (at, rule) in config.rules.iter().enumerate() {
        let _ = writeln!(out, "  {}. {}", at + 1, rule_line(rule));
    }
    out
}

fn rule_line(rule: &nullradix_api::RuleDto) -> String {
    let mut conditions = Vec::new();
    if let Some(labels) = &rule.when.labels {
        conditions.push(format!("labels {}", labels.join("|")));
    }
    if let Some(priorities) = &rule.when.priorities {
        conditions.push(format!("priority {}", priorities.join("|")));
    }
    if let Some(files) = rule.when.files_min {
        conditions.push(format!("files >= {files}"));
    }
    if let Some(chars) = rule.when.plan_min_chars {
        conditions.push(format!("plan >= {chars} chars"));
    }
    format!("{} -> {}", conditions.join(", "), rule.profile)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn merging_replaces_leaves_and_keeps_the_rest() {
        let mut config = json!({ "wip": 1, "claude": { "command": "claude", "maxTurns": 80 }, "ladder": ["a", "b"] });
        merge(
            &mut config,
            json!({ "wip": 3, "claude": { "maxTurns": 90 }, "ladder": ["c"], "window": null }),
        );
        assert_eq!(
            config,
            json!({ "wip": 3, "claude": { "command": "claude", "maxTurns": 90 }, "ladder": ["c"], "window": null })
        );
    }

    #[test]
    fn nothing_to_change_is_an_error_before_the_daemon_is_asked() {
        let args = SetArgs {
            file: None,
            enabled: None,
            wip: None,
        };
        let client = Client::new(
            "127.0.0.1:9".parse().unwrap(),
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(10),
        );
        let error = set(&client, &args).unwrap_err();
        assert!(error.to_string().contains("nothing to change"));
    }
}
