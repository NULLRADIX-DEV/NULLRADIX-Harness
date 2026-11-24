use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::Subcommand;
use nullradix_api::{BoardDto, CreateBoard, ProjectDto, RenameBoard};
use nullradix_client::Client;
use uuid::Uuid;

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
            let projects = match project {
                Some(key) => vec![find_project(client, &key)?],
                None => client.list_projects()?.items,
            };
            let mut rows = Vec::new();
            for project in &projects {
                for board in client.list_boards(project.id)?.items {
                    rows.push((project.key.clone(), board));
                }
            }
            if json {
                let boards: Vec<&BoardDto> = rows.iter().map(|(_, board)| board).collect();
                println!("{}", serde_json::to_string_pretty(&boards)?);
            } else if rows.is_empty() {
                println!("no boards yet, create one with `nullradix board create KEY NAME`");
            } else {
                for (key, board) in &rows {
                    println!(
                        "{}  {key:<6}  {}  ({} open)",
                        board.id, board.name, board.open_tickets
                    );
                }
            }
        }
        BoardCommand::Create { project, name } => {
            let project = find_project(client, &project)?;
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
            let project = find_project(client, &project)?;
            let current = find_board(client, &project, board)?;
            let renamed = client.rename_board(
                project.id,
                board,
                &RenameBoard {
                    name,
                    version: current.version,
                },
            )?;
            println!("renamed board {} to {}", renamed.id, renamed.name);
        }
        BoardCommand::Delete { project, board } => {
            let project = find_project(client, &project)?;
            let current = find_board(client, &project, board)?;
            client
                .delete_board(project.id, board, current.version)
                .with_context(|| format!("cannot delete board {board}"))?;
            println!("deleted board {board}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn find_project(client: &Client, key: &str) -> anyhow::Result<ProjectDto> {
    let wanted = key.to_ascii_uppercase();
    match client
        .list_projects()?
        .items
        .into_iter()
        .find(|project| project.key == wanted)
    {
        Some(project) => Ok(project),
        None => bail!("no project has the key {key}"),
    }
}

fn find_board(client: &Client, project: &ProjectDto, id: Uuid) -> anyhow::Result<BoardDto> {
    match client
        .list_boards(project.id)?
        .items
        .into_iter()
        .find(|board| board.id == id)
    {
        Some(board) => Ok(board),
        None => bail!("project {} has no board {id}", project.key),
    }
}
