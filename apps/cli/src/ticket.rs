use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context;
use clap::Subcommand;
use nullradix_api::{
    AcceptTicket, ClaimTicket, CleanedWorktree, ColumnDto, CreateTicket, FinishTicket,
    FinishedTicket, MoveTicket, PriorityDto, ReleaseTicket, StartTicket, StartedTicket,
    TicketDetailDto, TicketDto, UpdateTicket,
};
use nullradix_client::Client;
use nullradix_domain::{Priority, TicketKey, TicketStatus, Timestamp};
use uuid::Uuid;

use crate::board::local_runner;
use crate::project;
use crate::selection::{BoardChoice, BoardOption};

#[derive(Subcommand)]
pub(crate) enum TicketCommand {
    /// List tickets by column in display order
    List(ListArgs),
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
    /// Change fields of a ticket
    Set(Box<SetArgs>),
    /// Move a ticket to another column
    Move {
        /// Ticket key such as ARC-7
        key: String,
        /// backlog, in-planning, planned, in-progress, blocked, done or not-needed
        status: String,
        /// How to tell the ticket is done, set in the same step
        #[arg(long)]
        acceptance: Option<String>,
        /// Version the move is based on
        #[arg(long)]
        version: Option<i64>,
        /// Lift the dependency, question and not-needed locks
        #[arg(long)]
        force: bool,
    },
    /// Take a ticket for an agent; it moves to in progress unless told
    Claim {
        key: String,
        /// Name of the agent that works on it
        #[arg(long)]
        agent: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        version: Option<i64>,
        /// Take it from another assignee and lift the locks
        #[arg(long)]
        force: bool,
    },
    /// Start a ticket in its own worktree of the current folder's repository, or join one
    Start {
        /// Ticket key such as ARC-7
        key: String,
        /// Join the worktree of this ticket; another target branch gives an own worktree
        #[arg(long, conflicts_with = "join")]
        join_ticket: Option<String>,
        /// Join the first open worktree with the same target branch
        #[arg(long)]
        join: bool,
        /// Lift the dependency and question locks
        #[arg(long)]
        force: bool,
    },
    /// Finish a ticket: done, its worktree ready to merge and cleaned up; nothing is merged
    Done {
        /// Ticket key such as ARC-7
        key: String,
        /// How to tell the ticket is done, set in the same step
        #[arg(long, conflicts_with = "acceptance_file")]
        acceptance: Option<String>,
        /// Read the acceptance criteria from this file
        #[arg(long)]
        acceptance_file: Option<PathBuf>,
        /// Lift the dependency, question and not-needed locks
        #[arg(long)]
        force: bool,
    },
    /// Give a ticket back; a given status applies without locks
    Release {
        key: String,
        #[arg(long)]
        status: Option<String>,
    },
    /// Finish a ticket that waits for a person
    Accept {
        key: String,
        #[arg(long)]
        version: Option<i64>,
    },
    /// Delete a ticket for good
    Delete {
        /// Ticket key such as ARC-7
        key: String,
        /// Version the deletion is based on
        #[arg(long)]
        version: Option<i64>,
    },
}

#[derive(clap::Args)]
pub(crate) struct SetArgs {
    /// Ticket key such as ARC-7
    key: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long, conflicts_with = "description_file")]
    description: Option<String>,
    #[arg(long)]
    description_file: Option<PathBuf>,
    #[arg(long, conflicts_with = "plan_file")]
    plan: Option<String>,
    #[arg(long)]
    plan_file: Option<PathBuf>,
    #[arg(long, conflicts_with = "handbook_file")]
    handbook: Option<String>,
    #[arg(long)]
    handbook_file: Option<PathBuf>,
    /// Mermaid diagram; a fence around it is kept as given
    #[arg(long, conflicts_with = "diagram_file")]
    diagram: Option<String>,
    #[arg(long)]
    diagram_file: Option<PathBuf>,
    #[arg(long)]
    status: Option<String>,
    #[arg(long)]
    priority: Option<String>,
    /// Name of the assignee; empty or none clears it
    #[arg(long)]
    assignee: Option<String>,
    /// Replaces the labels; one flag may hold several split by commas
    #[arg(long = "label")]
    labels: Vec<String>,
    /// Why the ticket is blocked; empty clears it
    #[arg(long)]
    blocked_reason: Option<String>,
    #[arg(long, conflicts_with = "acceptance_file")]
    acceptance: Option<String>,
    #[arg(long)]
    acceptance_file: Option<PathBuf>,
    /// true or false; anything but false counts as true
    #[arg(long)]
    needs_planning: Option<String>,
    /// Replaces the dependencies with these comma-separated keys; none clears them
    #[arg(long, conflicts_with_all = ["depends_on_add", "depends_on_remove"])]
    depends_on: Option<String>,
    /// Adds dependencies
    #[arg(long)]
    depends_on_add: Vec<String>,
    /// Removes dependencies
    #[arg(long)]
    depends_on_remove: Vec<String>,
    /// Branch to merge into; empty or none means the default branch
    #[arg(long)]
    target_branch: Option<String>,
    /// Clear branch and worktree on the ticket; the branch and folder themselves stay
    #[arg(long)]
    clear_branch: bool,
    /// Board of the same project to move the ticket to
    #[arg(long)]
    to_board: Option<Uuid>,
    /// Version the change is based on; without it the current one is used
    #[arg(long)]
    version: Option<i64>,
    /// Lift the dependency, question and not-needed locks
    #[arg(long)]
    force: bool,
}

#[derive(clap::Args)]
pub(crate) struct ListArgs {
    /// A board id or `all`; by default the repository's board, else the active one
    #[arg(long)]
    board: Option<BoardOption>,
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
}

#[derive(clap::Args)]
pub(crate) struct CreateArgs {
    title: String,
    /// Board to create on; by default the repository's board, else the active one
    #[arg(long)]
    board: Option<Uuid>,
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
    /// A work step; repeat the flag for more
    #[arg(long = "step")]
    steps: Vec<String>,
    /// Lift the dependency, question and not-needed locks and allow a duplicate title
    #[arg(long)]
    force: bool,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

/// The value, or none for a literal `none` as legacy read it.
fn none_or(value: &str) -> Option<String> {
    (!value.trim().eq_ignore_ascii_case("none")).then(|| value.to_owned())
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

pub(crate) fn text(inline: Option<String>, file: Option<&Path>) -> anyhow::Result<Option<String>> {
    match file {
        Some(path) => fs::read_to_string(path)
            .map(Some)
            .with_context(|| format!("cannot read {}", path.display())),
        None => Ok(inline),
    }
}

pub(crate) fn run(
    command: TicketCommand,
    client: &Client,
    choice: &BoardChoice,
) -> anyhow::Result<ExitCode> {
    match command {
        TicketCommand::List(args) => list(&args, client, choice)?,
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
        TicketCommand::Create(args) => create(args, client, choice)?,
        TicketCommand::Set(args) => set(*args, client)?,
        TicketCommand::Move {
            key,
            status,
            acceptance,
            version,
            force,
        } => {
            let key = TicketKey::parse(&key)?.to_string();
            let input = MoveTicket {
                status: Some(status),
                version,
                acceptance,
                force,
            };
            let ticket = client.move_ticket(&key, &input)?;
            println!(
                "{} -> {}",
                ticket.key,
                TicketStatus::from(ticket.status).as_str()
            );
        }
        TicketCommand::Claim {
            key,
            agent,
            status,
            version,
            force,
        } => {
            let input = ClaimTicket {
                agent: agent.clone(),
                status,
                version,
                force,
            };
            let ticket = client.claim_ticket(&canonical(&key)?, &input)?;
            let name = ticket.assignee.clone().unwrap_or(agent);
            report(&ticket, &format!("claimed by {name}"));
        }
        TicketCommand::Start {
            key,
            join_ticket,
            join,
            force,
        } => start(client, &key, join_ticket.as_deref(), join, force)?,
        TicketCommand::Done {
            key,
            acceptance,
            acceptance_file,
            force,
        } => {
            let acceptance = text(acceptance, acceptance_file.as_deref())?;
            done(client, &key, acceptance, force)?;
        }
        TicketCommand::Release { key, status } => {
            let ticket = client.release_ticket(&canonical(&key)?, &ReleaseTicket { status })?;
            report(&ticket, "released");
        }
        TicketCommand::Accept { key, version } => {
            let input = AcceptTicket { version };
            report(
                &client.accept_ticket(&canonical(&key)?, &input)?,
                "accepted",
            );
        }
        TicketCommand::Delete { key, version } => {
            let key = TicketKey::parse(&key)?.to_string();
            client.delete_ticket(&key, version)?;
            println!("{key} deleted.");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn start(
    client: &Client,
    key: &str,
    join_ticket: Option<&str>,
    join: bool,
    force: bool,
) -> anyhow::Result<()> {
    let folder = std::env::current_dir().context("cannot read the current folder")?;
    let input = StartTicket {
        runner_id: local_runner(client)?.id,
        cwd: folder.to_string_lossy().into_owned(),
        join_ticket: join_ticket.map(canonical).transpose()?,
        join,
        force,
    };
    let started = client.start_ticket(&canonical(key)?, &input)?;
    for warning in &started.warnings {
        eprintln!("warning: {warning}");
    }
    for line in start_lines(&started) {
        println!("{line}");
    }
    Ok(())
}

fn done(client: &Client, key: &str, acceptance: Option<String>, force: bool) -> anyhow::Result<()> {
    let folder = std::env::current_dir().context("cannot read the current folder")?;
    let input = FinishTicket {
        runner_id: local_runner(client)?.id,
        cwd: folder.to_string_lossy().into_owned(),
        acceptance,
        version: None,
        force,
    };
    let finished = client.finish_ticket(&canonical(key)?, &input)?;
    for line in done_lines(&finished) {
        println!("{line}");
    }
    Ok(())
}

fn done_lines(finished: &FinishedTicket) -> Vec<String> {
    let mut lines = vec![format!("{} is done.", finished.ticket.key)];
    if finished.ready_to_merge
        && let Some(branch) = &finished.branch
    {
        lines.push(format!(
            "Branch {branch} is ready to merge (nothing was merged)."
        ));
    }
    lines.extend(cleanup_lines(&finished.cleanup));
    lines
}

/// One line per ready worktree the cleanup looked at.
fn cleanup_lines(cleanup: &[CleanedWorktree]) -> Vec<String> {
    cleanup
        .iter()
        .map(|cleaned| match &cleaned.kept_because {
            None => format!("Worktree {} removed (the branch stays).", cleaned.path),
            Some(reason) => format!("Worktree {} kept: {reason}.", cleaned.path),
        })
        .collect()
}

fn start_lines(started: &StartedTicket) -> Vec<String> {
    let how = if started.joined {
        "joined"
    } else {
        "started on"
    };
    let mut lines = vec![format!(
        "{} {how} {} in {}",
        started.ticket.key, started.branch, started.worktree
    )];
    if let Some(created) = &started.target_created {
        lines.push(format!(
            "created target branch {} from {}",
            created.branch, created.from
        ));
    }
    lines.extend(cleanup_lines(&started.cleanup));
    lines
}

fn list(args: &ListArgs, client: &Client, choice: &BoardChoice) -> anyhow::Result<()> {
    let project = match &args.project {
        Some(key) => Some(project::find(client, key)?.id.to_string()),
        None => None,
    };
    // a project alone names the scope; otherwise the board choice applies
    let board = if project.is_some() && args.board.is_none() {
        None
    } else {
        choice.many(client, args.board)?.map(|id| id.to_string())
    };
    let optional = [
        ("projectId", project.as_deref()),
        ("boardId", board.as_deref()),
        ("columns", args.status.as_deref()),
        ("assignee", args.assignee.as_deref()),
        ("label", args.label.as_deref()),
        ("query", args.query.as_deref()),
    ];
    let mut pairs: Vec<(&str, &str)> = optional
        .into_iter()
        .filter_map(|(name, value)| Some((name, value?)))
        .collect();
    if args.needs_planning {
        pairs.push(("needsPlanning", "true"));
    }
    let tickets = client.list_tickets(&pairs)?.items;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&tickets)?);
    } else {
        for line in list_lines(&tickets) {
            println!("{line}");
        }
    }
    Ok(())
}

pub(crate) fn canonical(key: &str) -> anyhow::Result<String> {
    Ok(TicketKey::parse(key)?.to_string())
}

/// One line such as `ARC-0001 released (backlog)`.
pub(crate) fn report(ticket: &TicketDto, what: &str) {
    println!(
        "{} {what} ({})",
        ticket.key,
        TicketStatus::from(ticket.status).as_str()
    );
}

fn set(args: SetArgs, client: &Client) -> anyhow::Result<()> {
    let key = TicketKey::parse(&args.key)?.to_string();
    let version = args.version;
    let keys = |texts: Vec<String>| {
        (!texts.is_empty()).then(|| {
            texts
                .iter()
                .flat_map(|text| dependency_list(text))
                .collect()
        })
    };
    let mut input = UpdateTicket {
        version: version.unwrap_or(1),
        title: args.title,
        description: text(args.description, args.description_file.as_deref())?,
        plan: text(args.plan, args.plan_file.as_deref())?,
        handbook: text(args.handbook, args.handbook_file.as_deref())?,
        diagram: text(args.diagram, args.diagram_file.as_deref())?,
        status: args.status,
        priority: args.priority,
        labels: (!args.labels.is_empty()).then_some(args.labels),
        assignee: args.assignee.as_deref().map(none_or),
        needs_planning: args
            .needs_planning
            .map(|value| !value.trim().eq_ignore_ascii_case("false")),
        blocked_reason: args.blocked_reason.map(Some),
        acceptance: text(args.acceptance, args.acceptance_file.as_deref())?,
        depends_on: args.depends_on.as_deref().map(dependency_list),
        depends_on_add: keys(args.depends_on_add),
        depends_on_remove: keys(args.depends_on_remove),
        board_id: args.to_board,
        target_branch: args.target_branch.as_deref().map(none_or),
        branch: args.clear_branch.then_some(None),
        worktree: args.clear_branch.then_some(None),
        force: args.force,
    };
    // without fields the daemon answers nothing-changed before it looks at the version
    let names_a_field = input
        != UpdateTicket {
            version: input.version,
            force: input.force,
            ..UpdateTicket::default()
        };
    if version.is_none() && names_a_field {
        input.version = client.get_ticket(&key)?.ticket.version;
    }
    let detail = client.update_ticket(&key, &input)?;
    println!(
        "{} changed (version {})",
        detail.ticket.key, detail.ticket.version
    );
    Ok(())
}

fn create(args: CreateArgs, client: &Client, choice: &BoardChoice) -> anyhow::Result<()> {
    let board = choice.board(client, args.board)?;
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
        steps: args.steps,
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
    if ticket.steps_total != 0 {
        marks.push(format!(
            "[{}/{} steps]",
            ticket.steps_done, ticket.steps_total
        ));
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
    lines.extend(relation_lines(ticket));
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
    if !detail.steps.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "## Steps ({}/{})",
            ticket.steps_done, ticket.steps_total
        ));
        for step in &detail.steps {
            let mark = if step.done { "x" } else { " " };
            lines.push(format!("  [{mark}] {}. {}", step.number, step.text));
            if let Some(guidance) = &step.detail {
                lines.extend(guidance.lines().map(|line| format!("        {line}")));
            }
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

/// Dependencies, dependents and the branches of the ticket.
fn relation_lines(ticket: &TicketDto) -> Vec<String> {
    let mut lines = Vec::new();
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
    if let Some(branch) = &ticket.branch {
        let folder = ticket
            .worktree
            .as_ref()
            .map_or_else(String::new, |path| format!(" (worktree {path})"));
        lines.push(format!("Branch: {branch}{folder}"));
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
