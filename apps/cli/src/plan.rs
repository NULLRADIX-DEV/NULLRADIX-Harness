use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullradix_api::{ClaimPlanning, NextPlanningClaim, WritePlan};
use nullradix_client::Client;
use uuid::Uuid;

use crate::project;
use crate::ticket::{canonical, report, text};

#[derive(Subcommand)]
pub(crate) enum PlanCommand {
    /// Write the plan of a ticket; it moves to planned unless told
    Write(WriteArgs),
    /// Show the next ticket that needs planning, or claim it with --claim
    Next {
        /// Claim it for this agent for 15 minutes
        #[arg(long)]
        claim: Option<String>,
        /// Only this board
        #[arg(long)]
        board: Option<Uuid>,
        /// Only this project key
        #[arg(long)]
        project: Option<String>,
    },
    /// Claim planning of a ticket for 15 minutes
    Claim {
        key: String,
        #[arg(long)]
        agent: String,
    },
    /// Give up a planning claim
    Release {
        key: String,
        #[arg(long)]
        agent: String,
    },
}

#[derive(clap::Args)]
pub(crate) struct WriteArgs {
    key: String,
    #[arg(long, conflicts_with = "plan_file")]
    plan: Option<String>,
    #[arg(long)]
    plan_file: Option<PathBuf>,
    #[arg(long, conflicts_with = "handbook_file")]
    handbook: Option<String>,
    #[arg(long)]
    handbook_file: Option<PathBuf>,
    /// Mermaid; a Markdown fence around it is dropped
    #[arg(long, conflicts_with = "diagram_file")]
    diagram: Option<String>,
    #[arg(long)]
    diagram_file: Option<PathBuf>,
    /// Kept as it is when left out or blank
    #[arg(long)]
    acceptance: Option<String>,
    #[arg(long)]
    status: Option<String>,
}

pub(crate) fn run(command: PlanCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        PlanCommand::Write(args) => {
            let input = WritePlan {
                plan: text(args.plan, args.plan_file.as_deref())?,
                handbook: text(args.handbook, args.handbook_file.as_deref())?,
                diagram: text(args.diagram, args.diagram_file.as_deref())?,
                acceptance: args.acceptance,
                status: args.status,
            };
            report(
                &client.write_plan(&canonical(&args.key)?, &input)?,
                "planned",
            );
        }
        PlanCommand::Next {
            claim,
            board,
            project,
        } => {
            let project = match project {
                Some(key) => Some(project::find(client, &key)?.id),
                None => None,
            };
            let next = match claim {
                Some(agent) => client.claim_next_planning(&NextPlanningClaim {
                    agent,
                    project_id: project,
                    board_id: board,
                })?,
                None => client.next_planning(project, board)?,
            };
            match next.ticket {
                Some(ticket) => println!("{}  {}", ticket.key, ticket.title),
                None => println!("No ticket needs planning."),
            }
        }
        PlanCommand::Claim { key, agent } => {
            let ticket = client.claim_planning(&canonical(&key)?, &ClaimPlanning { agent })?;
            report(&ticket, "claimed for planning");
        }
        PlanCommand::Release { key, agent } => {
            let key = canonical(&key)?;
            client.release_planning(&key, &agent)?;
            println!("{key} no longer claimed for planning by {agent}");
        }
    }
    Ok(ExitCode::SUCCESS)
}
