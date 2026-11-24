use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::Subcommand;
use nullradix_api::{ColumnDto, CreateTicket, PriorityDto, TicketDetailDto, TicketDto};
use nullradix_client::Client;
use nullradix_domain::{Priority, TicketKey, TicketStatus, Timestamp};
use uuid::Uuid;

use crate::project;

#[derive(Subcommand)]
pub(crate) enum TicketCommand {
    /// List tickets by column in display order
    List {
        /// Only the tickets of this board
        #[arg(long)]
        board: Option<Uuid>,
        /// Only the tickets of this project key
        #[arg(long)]
        project: Option<String>,
        /// Comma-separated columns, such as waiting,planned
        #[arg(long)]
        status: Option<String>,
        /// Only tickets that still need planning
        #[arg(long)]
        needs_planning: bool,
        /// Only tickets assigned to this name
        #[arg(long)]
        assignee: Option<String>,
        /// Only tickets with this label
        #[arg(long)]
        label: Option<String>,
        /// Text in the key, title or description
        #[arg(long)]
        query: Option<String>,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Show a ticket
    Show {
        /// Ticket key such as ARC-7
        key: String,
        /// Add the history of changes
        #[arg(long)]
        history: bool,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Create a ticket on a board
    Create(CreateArgs),
}

#[derive(clap::Args)]
pub(crate) struct CreateArgs {
    title: String,
    /// Board to create the ticket on
    #[arg(long)]
    board: Uuid,
    #[arg(long, conflicts_with = "description_file")]
    description: Option<String>,
    /// Read the description from this file
    #[arg(long)]
    description_file: Option<PathBuf>,
    /// Status to start in, backlog when left out
    #[arg(long)]
    status: Option<String>,
    /// low, normal, high or urgent
    #[arg(long)]
    priority: Option<String>,
    /// Labels; one flag may hold several split by commas
    #[arg(long = "label")]
    labels: Vec<String>,
    /// Comma-separated keys of tickets that must be done first
    #[arg(long)]
    depends_on: Option<String>,
    /// How to tell the ticket is done
    #[arg(long, conflicts_with = "acceptance_file")]
    acceptance: Option<String>,
    /// Read the acceptance criteria from this file
    #[arg(long)]
    acceptance_file: Option<PathBuf>,
    /// The ticket is planned already and needs no planning
    #[arg(long)]
    planned: bool,
    /// Branch to merge into instead of the default branch
    #[arg(long)]
    target_branch: Option<String>,
    /// Lift the dependency, question and not-needed locks and allow a duplicate title
    #[arg(long)]
    force: bool,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

/// Comma-separated ticket keys; blank entries drop out and `none` means no dependency.
fn dependency_list(text: &str) -> Vec<String> {
    if text.trim().eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    text.split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .collect()
}

fn text(inline: Option<String>, file: Option<&Path>) -> anyhow::Result<Option<String>> {
    match file {
        Some(path) => fs::read_to_string(path)
            .map(Some)
            .with_context(|| format!("cannot read {}", path.display())),
        None => Ok(inline),
    }
}

pub(crate) fn run(command: TicketCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        TicketCommand::List {
            board,
            project,
            status,
            needs_planning,
            assignee,
            label,
            query,
            json,
        } => {
            let project = match project {
                Some(key) => Some(project::find(client, &key)?.id.to_string()),
                None => None,
            };
            let board = board.map(|id| id.to_string());
            let mut pairs: Vec<(&str, &str)> = Vec::new();
            let optional = [
                ("projectId", project.as_deref()),
                ("boardId", board.as_deref()),
                ("columns", status.as_deref()),
                ("assignee", assignee.as_deref()),
                ("label", label.as_deref()),
                ("query", query.as_deref()),
            ];
            pairs.extend(
                optional
                    .into_iter()
                    .filter_map(|(name, value)| Some((name, value?))),
            );
            if needs_planning {
                pairs.push(("needsPlanning", "true"));
            }
            let tickets = client.list_tickets(&pairs)?.items;
            if json {
                println!("{}", serde_json::to_string_pretty(&tickets)?);
            } else {
                for line in list_lines(&tickets) {
                    println!("{line}");
                }
            }
        }
        TicketCommand::Show { key, history, json } => {
            let key = TicketKey::parse(&key)?;
            let detail = client.get_ticket(&key.to_string())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&detail)?);
            } else {
                for line in show_lines(&detail, history) {
                    println!("{line}");
                }
            }
        }
        TicketCommand::Create(args) => create(args, client)?,
    }
    Ok(ExitCode::SUCCESS)
}

fn create(args: CreateArgs, client: &Client) -> anyhow::Result<()> {
    let Some(board) = client
        .list_all_boards()?
        .items
        .into_iter()
        .find(|board| board.id == args.board)
    else {
        bail!("no board has the id {}", args.board);
    };
    let input = CreateTicket {
        title: args.title,
        description: text(args.description, args.description_file.as_deref())?,
        acceptance: text(args.acceptance, args.acceptance_file.as_deref())?,
        status: args.status,
        priority: args.priority,
        labels: args.labels,
        depends_on: args
            .depends_on
            .as_deref()
            .map_or_else(Vec::new, dependency_list),
        needs_planning: args.planned.then_some(false),
        target_branch: args.target_branch,
        force: args.force,
        allow_duplicate: args.force,
    };
    let ticket = client.create_ticket(board.project_id, board.id, &input)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&ticket)?);
    } else {
        println!("{} created: {}", ticket.key, ticket.title);
    }
    Ok(())
}

const COLUMNS: [(ColumnDto, &str); 8] = [
    (ColumnDto::Backlog, "Backlog"),
    (ColumnDto::Waiting, "Waiting"),
    (ColumnDto::InPlanning, "In planning"),
    (ColumnDto::Planned, "Planned"),
    (ColumnDto::InProgress, "In progress"),
    (ColumnDto::Blocked, "Blocked"),
    (ColumnDto::Done, "Done"),
    (ColumnDto::NotNeeded, "Not needed"),
];

/// Columns with their tickets in the order the daemon sent them.
fn list_lines(tickets: &[TicketDto]) -> Vec<String> {
    if tickets.is_empty() {
        return vec![String::from("(no tickets found)")];
    }
    let mut lines = Vec::new();
    for (column, label) in COLUMNS {
        let in_column: Vec<&TicketDto> = tickets
            .iter()
            .filter(|ticket| ticket.column == column)
            .collect();
        if in_column.is_empty() {
            continue;
        }
        lines.push(String::new());
        lines.push(format!("{label} ({})", in_column.len()));
        for ticket in in_column {
            let marks = marks(ticket);
            let tail = if marks.is_empty() {
                String::new()
            } else {
                format!("  {}", marks.join(" "))
            };
            lines.push(format!("  {}  {}{tail}", ticket.key, ticket.title));
        }
    }
    lines
}

fn marks(ticket: &TicketDto) -> Vec<String> {
    let mut marks = Vec::new();
    if ticket.waiting {
        marks.push(format!("[waiting for {}]", ticket.depends_open.join(", ")));
    }
    if ticket.needs_planning {
        marks.push(String::from("[planning]"));
    }
    if let Some(assignee) = &ticket.assignee {
        marks.push(format!("@{assignee}"));
    }
    if ticket.priority != PriorityDto::Normal {
        marks.push(format!("({})", priority_name(ticket.priority)));
    }
    if !ticket.labels.is_empty() {
        marks.push(ticket.labels.join(","));
    }
    marks
}

fn priority_name(priority: PriorityDto) -> &'static str {
    Priority::from(priority).as_str()
}

fn show_lines(detail: &TicketDetailDto, history: bool) -> Vec<String> {
    let ticket = &detail.ticket;
    let mut head = vec![
        format!(
            "Status {}{}",
            TicketStatus::from(ticket.status).as_str(),
            if ticket.waiting { " (waiting)" } else { "" }
        ),
        format!("Priority {}", priority_name(ticket.priority)),
        format!("version {}", ticket.version),
        ticket
            .assignee
            .as_ref()
            .map_or_else(|| String::from("unassigned"), |name| format!("@{name}")),
    ];
    if ticket.needs_planning {
        head.push(String::from("needs planning"));
    }
    if !ticket.labels.is_empty() {
        head.push(ticket.labels.join(","));
    }
    let mut lines = vec![
        format!("{}  {}", ticket.key, ticket.title),
        head.join(" | "),
    ];
    if let Some(reason) = &ticket.blocked_reason {
        lines.push(format!("Blocked: {reason}"));
    }
    if !ticket.acceptance.is_empty() {
        lines.push(format!("Acceptance: {}", ticket.acceptance));
    }
    if !ticket.depends_on.is_empty() {
        let states: Vec<String> = ticket
            .depends_on
            .iter()
            .map(|key| {
                let state = if ticket.depends_open.contains(key) {
                    "open"
                } else {
                    "met"
                };
                format!("{key} ({state})")
            })
            .collect();
        lines.push(format!("Depends on: {}", states.join(", ")));
    }
    if !ticket.dependents.is_empty() {
        lines.push(format!("Needed by: {}", ticket.dependents.join(", ")));
    }
    if let Some(target) = &ticket.target_branch {
        lines.push(format!("Target branch: {target}"));
    }
    let sections = [
        ("Description", &ticket.description),
        ("Plan", &ticket.plan),
        ("Handbook", &ticket.handbook),
        ("Diagram (mermaid)", &ticket.diagram),
    ];
    for (title, text) in sections {
        if !text.trim().is_empty() {
            lines.push(String::new());
            lines.push(format!("## {title}"));
            lines.push(text.trim_end().to_owned());
        }
    }
    if history {
        lines.push(String::new());
        lines.push(String::from("## History"));
        for entry in &detail.history {
            let change = match (&entry.from, &entry.to) {
                (Some(from), Some(to)) => format!("{}: {from} -> {to}", entry.field),
                (None, Some(to)) => format!("{}: {to}", entry.field),
                (Some(from), None) => format!("{}: {from} removed", entry.field),
                (None, None) => entry.field.clone(),
            };
            let note = entry
                .note
                .as_ref()
                .map_or_else(String::new, |note| format!(" ({note})"));
            lines.push(format!(
                "{}  {}  {change}{note}",
                Timestamp::from_millis(entry.at),
                entry.actor_name
            ));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_lists_drop_blanks_and_read_none_as_empty() {
        assert_eq!(dependency_list("ARC-1, ,arc-2,"), ["ARC-1", "arc-2"]);
        assert_eq!(dependency_list(" None "), Vec::<String>::new());
    }
}
