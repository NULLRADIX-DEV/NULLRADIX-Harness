use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::bail;
use clap::Subcommand;
use nullradix_api::{AddStep, ChangeStep, MoveStep, TicketDetailDto};
use nullradix_client::Client;
use uuid::Uuid;

use crate::comment::agent_session;
use crate::ticket::{canonical, text};

#[derive(Subcommand)]
pub(crate) enum StepCommand {
    /// Check a step, or uncheck it with --undo
    Check {
        /// Ticket key such as ARC-7
        key: String,
        /// Step number from 1
        number: u32,
        #[arg(long)]
        undo: bool,
        #[command(flatten)]
        guard: Guard,
    },
    /// Add a step at the end or at a position
    Add {
        key: String,
        text: String,
        #[arg(long, conflicts_with = "detail_file")]
        detail: Option<String>,
        /// Read the guidance from this file
        #[arg(long)]
        detail_file: Option<PathBuf>,
        /// Position from 1
        #[arg(long)]
        at: Option<i64>,
        #[command(flatten)]
        guard: Guard,
    },
    /// Change the text or guidance of a step; an empty detail removes the guidance
    Edit {
        key: String,
        number: u32,
        #[arg(long)]
        text: Option<String>,
        #[arg(long, conflicts_with = "detail_file")]
        detail: Option<String>,
        #[arg(long)]
        detail_file: Option<PathBuf>,
        #[command(flatten)]
        guard: Guard,
    },
    /// Remove a step; later ones move up
    Remove {
        key: String,
        number: u32,
        #[command(flatten)]
        guard: Guard,
    },
    /// Move a step to another number
    Move {
        key: String,
        from: u32,
        to: u32,
        #[command(flatten)]
        guard: Guard,
    },
}

/// What the change expects to find, so it never hits a ticket someone else changed.
#[derive(clap::Args)]
pub(crate) struct Guard {
    /// Version of the ticket the change is based on
    #[arg(long)]
    version: Option<i64>,
    /// Text the step had when it was read
    #[arg(long)]
    expect: Option<String>,
    /// Who changes it, such as claude; the person the daemon acts for when left out
    #[arg(long)]
    author: Option<String>,
}

pub(crate) fn run(command: StepCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        StepCommand::Check {
            key,
            number,
            undo,
            guard,
        } => {
            let key = canonical(&key)?;
            let input = ChangeStep {
                done: Some(!undo),
                version: guard.version,
                expect_text: guard.expect,
                author: guard.author,
                session: agent_session(),
                ..ChangeStep::default()
            };
            let detail = client.change_step(&key, step_id(client, &key, number)?, &input)?;
            println!(
                "{key}: {}/{} steps done.",
                detail.ticket.steps_done, detail.ticket.steps_total
            );
        }
        StepCommand::Add {
            key,
            text: step,
            detail,
            detail_file,
            at,
            guard,
        } => {
            let key = canonical(&key)?;
            let input = AddStep {
                text: step,
                detail: text(detail, detail_file.as_deref())?,
                at,
                version: guard.version,
                expect_text: guard.expect,
                author: guard.author,
                session: agent_session(),
            };
            let detail = client.add_step(&key, &input)?;
            let number = at.map_or(detail.ticket.steps_total.to_string(), |at| at.to_string());
            print(&detail, &format!("step {number} added"));
        }
        StepCommand::Edit {
            key,
            number,
            text: step,
            detail,
            detail_file,
            guard,
        } => {
            let key = canonical(&key)?;
            let detail = text(detail, detail_file.as_deref())?
                .map(|detail| (!detail.trim().is_empty()).then_some(detail));
            let input = ChangeStep {
                text: step,
                detail,
                version: guard.version,
                expect_text: guard.expect,
                author: guard.author,
                session: agent_session(),
                ..ChangeStep::default()
            };
            let detail = client.change_step(&key, step_id(client, &key, number)?, &input)?;
            print(&detail, &format!("step {number} changed"));
        }
        StepCommand::Remove { key, number, guard } => {
            let key = canonical(&key)?;
            let detail = client.remove_step(
                &key,
                step_id(client, &key, number)?,
                guard.version,
                guard.expect.as_deref(),
                guard.author.as_deref(),
                agent_session().as_deref(),
            )?;
            print(&detail, &format!("step {number} removed"));
        }
        StepCommand::Move {
            key,
            from,
            to,
            guard,
        } => {
            let key = canonical(&key)?;
            let input = MoveStep {
                to: i64::from(to),
                version: guard.version,
                expect_text: guard.expect,
                author: guard.author,
                session: agent_session(),
            };
            let detail = client.move_step(&key, step_id(client, &key, from)?, &input)?;
            print(&detail, &format!("step {from} -> {to}"));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The id of step `number` as the ticket reads now; later changes follow the step, not the number.
fn step_id(client: &Client, key: &str, number: u32) -> anyhow::Result<Uuid> {
    match client
        .get_ticket(key)?
        .steps
        .into_iter()
        .find(|step| step.number == number)
    {
        Some(step) => Ok(step.id),
        None => bail!("{key} has no step {number}"),
    }
}

fn print(detail: &TicketDetailDto, what: &str) {
    println!(
        "{}: {what} ({}/{} done).",
        detail.ticket.key, detail.ticket.steps_done, detail.ticket.steps_total
    );
    for step in &detail.steps {
        let mark = if step.done { "x" } else { " " };
        println!("  {}. [{mark}] {}", step.number, step.text);
    }
}
