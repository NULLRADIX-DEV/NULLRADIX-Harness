use std::process::ExitCode;

use clap::Subcommand;
use nullradix_api::{DisplayStateDto, SessionDto, SubagentStateDto};
use nullradix_client::Client;
use nullradix_domain::Timestamp;
use uuid::Uuid;

/// Characters of a session id lists show, as legacy did.
const SHORT_ID: usize = 8;

#[derive(Subcommand)]
pub(crate) enum SessionCommand {
    /// List Claude Code sessions, latest activity first
    #[command(
        after_help = "Examples:\n  nullradix session list --state running\n  nullradix session list --ticket ARC-7"
    )]
    List {
        /// Only sessions in this repository id
        #[arg(long)]
        repository: Option<Uuid>,
        /// idle, running, waiting, ended or stale
        #[arg(long)]
        state: Option<String>,
        /// Only sessions linked to this ticket key
        #[arg(long)]
        ticket: Option<String>,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Show one session with its subagents and tickets
    Show {
        /// Claude Code's session id
        session_id: String,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run(command: SessionCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        SessionCommand::List {
            repository,
            state,
            ticket,
            json,
        } => {
            let repository = repository.map(|id| id.to_string());
            let mut query = Vec::new();
            if let Some(repository) = repository.as_deref() {
                query.push(("repositoryId", repository));
            }
            if let Some(state) = state.as_deref() {
                query.push(("state", state));
            }
            if let Some(ticket) = ticket.as_deref() {
                query.push(("ticket", ticket));
            }
            let sessions = client.list_sessions(&query)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
            } else {
                for line in list_lines(&sessions.items) {
                    println!("{line}");
                }
            }
        }
        SessionCommand::Show { session_id, json } => {
            let session = client.get_session(&session_id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&session)?);
            } else {
                for line in show_lines(&session) {
                    println!("{line}");
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn state_label(state: DisplayStateDto) -> &'static str {
    match state {
        DisplayStateDto::Idle => "idle",
        DisplayStateDto::Running => "running",
        DisplayStateDto::Waiting => "waiting",
        DisplayStateDto::Ended => "ended",
        DisplayStateDto::Stale => "stale",
    }
}

/// `less than 1 min`, `5 min`, `1 h 05 min` or `2 d 3 h`, as the legacy list.
fn duration(millis: i64) -> String {
    let minutes = millis / 60_000;
    if minutes < 1 {
        return String::from("less than 1 min");
    }
    if minutes < 60 {
        return format!("{minutes} min");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours} h {:02} min", minutes % 60);
    }
    format!("{} d {} h", hours / 24, hours % 24)
}

fn short(id: &str) -> &str {
    id.char_indices()
        .nth(SHORT_ID)
        .map_or(id, |(end, _)| &id[..end])
}

fn title(session: &SessionDto) -> &str {
    if session.label.is_empty() {
        "(untitled)"
    } else {
        &session.label
    }
}

fn tickets(session: &SessionDto) -> String {
    session
        .tickets
        .iter()
        .map(|ticket| ticket.key.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// State, runtime, title, short id, tickets, last activity and folder; running subagents below.
fn list_lines(sessions: &[SessionDto]) -> Vec<String> {
    if sessions.is_empty() {
        return vec![String::from("No sessions.")];
    }
    let mut lines = Vec::new();
    for session in sessions {
        let linked = tickets(session);
        let linked = if linked.is_empty() {
            String::from("-")
        } else {
            linked
        };
        lines.push(format!(
            "{:<8} {:<16} {}  [{}]  {}  last {}  {}",
            state_label(session.display_state),
            duration(session.runtime_ms),
            title(session),
            short(&session.id),
            linked,
            Timestamp::from_millis(session.last_event_at),
            session.cwd,
        ));
        for agent in &session.subagents {
            if agent.state == SubagentStateDto::Running {
                lines.push(format!("    agent {} {}", agent.kind, agent.id));
            }
        }
    }
    lines
}

fn show_lines(session: &SessionDto) -> Vec<String> {
    let optional = |value: Option<&str>| value.unwrap_or("-").to_owned();
    let mut lines = vec![
        format!("{} ({})", title(session), session.id),
        format!("State:      {}", state_label(session.display_state)),
        format!("Runtime:    {}", duration(session.runtime_ms)),
        format!("Started:    {}", Timestamp::from_millis(session.started_at)),
        format!(
            "Last hook:  {}",
            Timestamp::from_millis(session.last_event_at)
        ),
        format!("Folder:     {}", session.cwd),
        format!("Model:      {}", optional(session.model.as_deref())),
    ];
    if let Some(ended) = session.ended_at {
        lines.insert(5, format!("Ended:      {}", Timestamp::from_millis(ended)));
    }
    let linked = tickets(session);
    if !linked.is_empty() {
        lines.push(format!("Tickets:    {linked}"));
    }
    if let Some(run) = &session.run_name {
        lines.push(format!("Run:        {run}"));
    }
    if !session.subagents.is_empty() {
        lines.push(String::from("Subagents:"));
        for agent in &session.subagents {
            let state = match agent.state {
                SubagentStateDto::Running => "running",
                SubagentStateDto::Ended => "ended",
            };
            lines.push(format!("  {} {} ({state})", agent.kind, agent.id));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use nullradix_api::{SessionClientDto, SessionStateDto, SubagentDto};

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: SessionCommand,
    }

    #[test]
    fn parses_list_filters_and_show() {
        let cli = Cli::try_parse_from(["session", "list", "--state", "running", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            SessionCommand::List { state: Some(state), json: true, .. } if state == "running"
        ));
        let cli = Cli::try_parse_from(["session", "show", "abc"]).unwrap();
        assert!(matches!(
            cli.command,
            SessionCommand::Show { session_id, json: false } if session_id == "abc"
        ));
        assert!(Cli::try_parse_from(["session", "list", "--repository", "x"]).is_err());
        assert!(Cli::try_parse_from(["session", "show"]).is_err());
    }

    #[test]
    fn durations_read_like_the_legacy_list() {
        assert_eq!(duration(59_000), "less than 1 min");
        assert_eq!(duration(5 * 60_000), "5 min");
        assert_eq!(duration(65 * 60_000), "1 h 05 min");
        assert_eq!(duration(51 * 60 * 60_000), "2 d 3 h");
    }

    fn session() -> SessionDto {
        SessionDto {
            id: String::from("0123456789abcdef"),
            repository_id: None,
            runner_id: None,
            number: Some(1),
            label: String::from("SE-001: Fix the login"),
            title: Some(String::from("Fix the login")),
            cwd: String::from("/repo"),
            state: SessionStateDto::Running,
            display_state: DisplayStateDto::Running,
            listening: true,
            started_at: 0,
            last_event_at: 60_000,
            ended_at: None,
            runtime_ms: 60_000,
            model: None,
            transcript_path: None,
            client: SessionClientDto::ClaudeCode,
            run_name: None,
            subagents: vec![SubagentDto {
                id: String::from("a1"),
                kind: String::from("Explore"),
                state: SubagentStateDto::Running,
                started_at: 0,
                ended_at: None,
            }],
            tickets: Vec::new(),
            version: 3,
        }
    }

    #[test]
    fn lists_one_line_per_session_with_running_agents_below() {
        assert_eq!(
            list_lines(&[session()]),
            [
                "running  1 min            SE-001: Fix the login  [01234567]  -  last 1970-01-01T00:01:00.000Z  /repo",
                "    agent Explore a1",
            ]
        );
        assert_eq!(list_lines(&[]), ["No sessions."]);
    }

    #[test]
    fn shows_the_session_in_full() {
        let lines = show_lines(&session());
        assert_eq!(lines[0], "SE-001: Fix the login (0123456789abcdef)");
        assert_eq!(
            lines.last().map(String::as_str),
            Some("  Explore a1 (running)")
        );
    }
}
