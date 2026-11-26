use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::Subcommand;
use nullradix_api::{ClaimPlanning, NextPlanningClaim, WritePlan};
use nullradix_client::Client;
use serde::Deserialize;

use crate::comment::agent_session;
use crate::project;
use crate::research::read_limited;
use crate::selection::{BoardChoice, BoardOption};
use crate::ticket::{canonical, report, text};

const IMPORT_FORMAT: &str = r#"File: {"plan": "...", "handbook": "...", "diagram": "...", "acceptance": "...", "status": "planned", "by": "planner-ARC-7"}
Example:
  nullradix plan import ARC-7 --file plan.json --author planner-ARC-7"#;

#[derive(Subcommand)]
pub(crate) enum PlanCommand {
    /// Write the plan of a ticket; it moves to planned unless told
    Write(WriteArgs),
    /// Save the plan a planning run wrote to plan.json
    #[command(after_help = IMPORT_FORMAT)]
    Import(ImportArgs),
    /// Start a planning run on a ticket
    Start {
        /// Ticket key such as ARC-7
        key: String,
    },
    /// Show the next ticket that needs planning, or claim it with --claim
    Next {
        /// Claim it for this agent for 15 minutes
        #[arg(long)]
        claim: Option<String>,
        /// A board id or `all`; by default the repository's board, else the active one
        #[arg(long)]
        board: Option<BoardOption>,
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
pub(crate) struct ImportArgs {
    key: String,
    /// JSON file with the plan; `-` reads standard input
    #[arg(long, alias = "from")]
    file: PathBuf,
    /// Who planned, such as planner-ARC-7; the person the daemon acts for when left out
    #[arg(long)]
    author: Option<String>,
    /// Target status when the file names none
    #[arg(long)]
    status: Option<String>,
}

/// The parts of a plan file; other fields are ignored as in legacy.
#[derive(Debug, Default, Deserialize)]
struct PlanFile {
    plan: Option<String>,
    handbook: Option<String>,
    diagram: Option<String>,
    acceptance: Option<String>,
    status: Option<String>,
    by: Option<String>,
}

/// The plan as written; the call names the author and the session when it can.
fn import_input(args: &ImportArgs, session: Option<String>) -> anyhow::Result<WritePlan> {
    let file: PlanFile = serde_json::from_str(&read_limited(&args.file, "plan file")?)
        .context("the plan file is not a JSON object whose fields are all text")?;
    Ok(WritePlan {
        plan: file.plan,
        handbook: file.handbook,
        diagram: file.diagram,
        acceptance: file.acceptance,
        status: file.status.or_else(|| args.status.clone()),
        author: args.author.clone().or(file.by),
        session,
    })
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

pub(crate) fn run(
    command: PlanCommand,
    client: &Client,
    choice: &BoardChoice,
) -> anyhow::Result<ExitCode> {
    match command {
        PlanCommand::Write(args) => {
            let input = WritePlan {
                plan: text(args.plan, args.plan_file.as_deref())?,
                handbook: text(args.handbook, args.handbook_file.as_deref())?,
                diagram: text(args.diagram, args.diagram_file.as_deref())?,
                acceptance: args.acceptance,
                status: args.status,
                author: None,
                session: None,
            };
            report(
                &client.write_plan(&canonical(&args.key)?, &input)?,
                "planned",
            );
        }
        PlanCommand::Import(args) => {
            let input = import_input(&args, agent_session())?;
            report(
                &client.write_plan(&canonical(&args.key)?, &input)?,
                "planned",
            );
        }
        PlanCommand::Start { key } => {
            let run = client.start_plan_run(&canonical(&key)?)?;
            println!("Planning started: run {}.", run.id);
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
            let board = if project.is_some() && board.is_none() {
                None
            } else {
                choice.many(client, board)?
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::research::FILE_MAX_BYTES;

    fn args(file: &std::path::Path, author: Option<&str>, status: Option<&str>) -> ImportArgs {
        ImportArgs {
            key: String::from("ARC-7"),
            file: file.to_path_buf(),
            author: author.map(str::to_owned),
            status: status.map(str::to_owned),
        }
    }

    fn file(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.json");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn the_file_gives_the_parts_and_the_call_names_the_author_and_session() {
        let json = r#"{"plan":"P","handbook":"H","diagram":"D","acceptance":"A","by":"someone","other":1}"#;
        let (_dir, path) = file(json);
        let input = import_input(
            &args(&path, Some("planner-ARC-7"), None),
            Some(String::from("s-1")),
        )
        .unwrap();
        assert_eq!(input.plan.as_deref(), Some("P"));
        assert_eq!(input.handbook.as_deref(), Some("H"));
        assert_eq!(input.diagram.as_deref(), Some("D"));
        assert_eq!(input.acceptance.as_deref(), Some("A"));
        assert_eq!(input.author.as_deref(), Some("planner-ARC-7"));
        assert_eq!(input.session.as_deref(), Some("s-1"));
        assert_eq!(input.status, None);
    }

    #[test]
    fn without_an_author_flag_the_file_names_one_and_the_file_status_wins() {
        let (_dir, path) = file(r#"{"plan":"P","by":"planner-ARC-7","status":"blocked"}"#);
        let input = import_input(&args(&path, None, Some("planned")), None).unwrap();
        assert_eq!(input.author.as_deref(), Some("planner-ARC-7"));
        assert_eq!(input.status.as_deref(), Some("blocked"));
        let (_dir, bare) = file(r#"{"plan":"P"}"#);
        let input = import_input(&args(&bare, None, Some("planned")), None).unwrap();
        assert_eq!(input.status.as_deref(), Some("planned"));
        assert_eq!(input.author, None);
    }

    #[test]
    fn a_file_that_is_no_object_of_texts_is_refused() {
        for text in ["{broken", "[]", r#"{"plan":7}"#, r#"{"by":false}"#] {
            let (_dir, path) = file(text);
            assert!(
                import_input(&args(&path, None, None), None).is_err(),
                "{text}"
            );
        }
    }

    #[test]
    fn a_file_just_above_the_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let exact = dir.path().join("exact.json");
        let pad = " ".repeat(usize::try_from(FILE_MAX_BYTES).unwrap() - 2);
        std::fs::write(&exact, format!("{{{pad}}}")).unwrap();
        assert!(import_input(&args(&exact, None, None), None).is_ok());
        let over = dir.path().join("over.json");
        let pad = " ".repeat(usize::try_from(FILE_MAX_BYTES).unwrap() - 1);
        std::fs::write(&over, format!("{{{pad}}}")).unwrap();
        let error = import_input(&args(&over, None, None), None).unwrap_err();
        assert!(error.to_string().contains("more than"), "{error}");
    }
}
