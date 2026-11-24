use std::process::ExitCode;

use anyhow::Context;
use clap::Subcommand;
use nullradix_api::{CreateBoard, RenameBoard};
use nullradix_client::Client;
use uuid::Uuid;

use crate::project;

#[derive(Subcommand)]
pub(crate) enum BoardCommand {
    /// List boards, of one project or of all
    List {
        /// Only the boards of this project key
        #[arg(long)]
        project: Option<String>,
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Create a board in a project
    Create {
        /// Project key such as ARC
        project: String,
        name: String,
    },
    /// Rename a board
    Rename {
        project: String,
        board: Uuid,
        name: String,
    },
    /// Delete a board
    Delete { project: String, board: Uuid },
}

pub(crate) fn run(command: BoardCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        BoardCommand::List { project, json } => {
            let boards = match project {
                Some(key) => client.list_boards(project::find(client, &key)?.id)?,
                None => client.list_all_boards()?,
            }
            .items;
            if json {
                println!("{}", serde_json::to_string_pretty(&boards)?);
            } else if boards.is_empty() {
                println!("no boards yet, create one with `nullradix board create KEY NAME`");
            } else {
                for board in &boards {
                    println!(
                        "{}  {:<6}  {}  ({} open)",
                        board.id, board.project_key, board.name, board.open_tickets
                    );
                }
            }
        }
        BoardCommand::Create { project, name } => {
            let project = project::find(client, &project)?;
            let board = client.create_board(project.id, &CreateBoard { name })?;
            println!(
                "created board {} {} in {}",
                board.id, board.name, project.key
            );
        }
        BoardCommand::Rename {
            project,
            board,
            name,
        } => {
            let project = project::find(client, &project)?;
            let current = client.get_board(project.id, board)?;
            let renamed = client
                .rename_board(
                    project.id,
                    board,
                    &RenameBoard {
                        name,
                        version: current.version,
                    },
                )
                .with_context(|| format!("cannot rename board {board}"))?;
            println!("renamed board {} to {}", renamed.id, renamed.name);
        }
        BoardCommand::Delete { project, board } => {
            let project = project::find(client, &project)?;
            let current = client.get_board(project.id, board)?;
            client
                .delete_board(project.id, board, current.version)
                .with_context(|| format!("cannot delete board {board}"))?;
            println!("deleted board {board}");
        }
    }
    Ok(ExitCode::SUCCESS)
}
