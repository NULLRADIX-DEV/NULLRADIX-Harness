use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::Subcommand;
use nullradix_api::{BoardDto, CreateBoard, RenameBoard, RunnerDto, SetBoardRepository};
use nullradix_client::Client;
use uuid::Uuid;

use crate::project;
use crate::selection::BoardChoice;

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
    Delete {
        project: String,
        board: Uuid,
        /// Also delete tickets that are neither done nor not needed
        #[arg(long)]
        delete_open: bool,
    },
    /// Make a board the default of commands run outside its repository
    Use {
        #[arg(required_unless_present = "clear")]
        board: Option<Uuid>,
        /// Have no active board
        #[arg(long, conflicts_with = "board")]
        clear: bool,
    },
    /// Show or set the repository a board's work happens in
    Repo {
        board: Uuid,
        /// A folder inside the repository's checkout on this machine
        path: Option<PathBuf>,
        /// Remove the repository from the board
        #[arg(long, conflicts_with = "path")]
        clear: bool,
    },
}

pub(crate) fn run(
    command: BoardCommand,
    client: &Client,
    choice: &BoardChoice,
) -> anyhow::Result<ExitCode> {
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
                let active = choice.active()?;
                for board in &boards {
                    let marker = if Some(board.id) == active {
                        "  (active)"
                    } else {
                        ""
                    };
                    let repository = board
                        .repository_name
                        .as_ref()
                        .map_or_else(String::new, |name| format!("  [{name}]"));
                    println!(
                        "{}  {:<6}  {}  ({} open){repository}{marker}",
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
        BoardCommand::Delete {
            project,
            board,
            delete_open,
        } => {
            let project = project::find(client, &project)?;
            let current = client.get_board(project.id, board)?;
            let deleted = client
                .delete_board(project.id, board, current.version, delete_open)
                .with_context(|| format!("cannot delete board {board}"))?;
            let count = deleted.deleted_tickets.len();
            let noun = if count == 1 { "ticket" } else { "tickets" };
            println!("deleted board {board} ({count} {noun})");
        }
        BoardCommand::Use { board, clear } => {
            use_board(client, choice, if clear { None } else { board })?;
        }
        BoardCommand::Repo { board, path, clear } => repo(client, board, path.as_deref(), clear)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Sets or clears the active board, as legacy `board boards use`.
fn use_board(client: &Client, choice: &BoardChoice, id: Option<Uuid>) -> anyhow::Result<()> {
    let Some(id) = id else {
        choice.set_active(None)?;
        println!("no active board");
        return Ok(());
    };
    let Some(board) = client
        .list_all_boards()?
        .items
        .into_iter()
        .find(|board| board.id == id)
    else {
        bail!("no board has the id {id}");
    };
    choice.set_active(Some(board.id))?;
    println!("active board: {} {}", board.id, board.name);
    Ok(())
}

/// Points the board to the repository around `path` on this machine.
pub(crate) fn bind_repository(
    client: &Client,
    board: &BoardDto,
    local: &RunnerDto,
    path: &Path,
) -> anyhow::Result<BoardDto> {
    let absolute =
        std::path::absolute(path).with_context(|| format!("cannot resolve {}", path.display()))?;
    let Some(text) = absolute.to_str() else {
        bail!("{} is not valid UTF-8", absolute.display());
    };
    let input = SetBoardRepository {
        runner_id: local.id,
        path: text.to_owned(),
        version: board.version,
    };
    client
        .set_board_repository(board.project_id, board.id, &input)
        .with_context(|| format!("cannot set the repository of board {}", board.id))
}

/// Shows, sets or clears the board's repository, as legacy `board boards repo`.
fn repo(client: &Client, id: Uuid, path: Option<&Path>, clear: bool) -> anyhow::Result<()> {
    let Some(board) = client
        .list_all_boards()?
        .items
        .into_iter()
        .find(|board| board.id == id)
    else {
        bail!("no board has the id {id}");
    };
    let local = local_runner(client)?;
    let board = if clear {
        client
            .clear_board_repository(board.project_id, id, board.version)
            .with_context(|| format!("cannot clear the repository of board {id}"))?
    } else if let Some(path) = path {
        bind_repository(client, &board, &local, path)?
    } else {
        board
    };
    println!("{}", describe(client, &board, &local)?);
    Ok(())
}

pub(crate) fn local_runner(client: &Client) -> anyhow::Result<RunnerDto> {
    match client
        .list_runners()?
        .items
        .into_iter()
        .find(|runner| runner.local)
    {
        Some(runner) => Ok(runner),
        None => bail!("the daemon has no local runner"),
    }
}

/// The board's repository with its checkout on this machine.
fn describe(client: &Client, board: &BoardDto, local: &RunnerDto) -> anyhow::Result<String> {
    let (Some(id), Some(name)) = (board.repository_id, board.repository_name.as_ref()) else {
        return Ok(format!("board {} has no repository", board.id));
    };
    let checkout = client
        .list_repositories()?
        .items
        .into_iter()
        .find(|repository| repository.id == id)
        .and_then(|repository| {
            repository
                .checkouts
                .into_iter()
                .find(|checkout| checkout.runner_id == local.id)
        });
    Ok(match checkout {
        Some(checkout) => format!("board {} works in {name} at {}", board.id, checkout.path),
        None => format!(
            "board {} works in {name}, not checked out on this machine",
            board.id
        ),
    })
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: BoardCommand,
    }

    #[test]
    fn repo_takes_a_path_or_clear_but_not_both() {
        let board = "0190a0b0-0000-7000-8000-000000000001";
        let parse = |args: &[&str]| Cli::try_parse_from([&["board", "repo", board], args].concat());
        assert!(parse(&[]).is_ok());
        assert!(parse(&["/work/arches"]).is_ok());
        assert!(parse(&["--clear"]).is_ok());
        let error = parse(&["/work/arches", "--clear"]).err().unwrap();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn use_takes_a_board_or_clear() {
        let parse = |args: &[&str]| Cli::try_parse_from([&["board", "use"], args].concat());
        assert!(parse(&["0190a0b0-0000-7000-8000-000000000001"]).is_ok());
        assert!(parse(&["--clear"]).is_ok());
        let missing = parse(&[]).err().unwrap();
        assert_eq!(
            missing.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        let both = parse(&["0190a0b0-0000-7000-8000-000000000001", "--clear"])
            .err()
            .unwrap();
        assert_eq!(both.kind(), clap::error::ErrorKind::ArgumentConflict);
    }
}
