use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::Subcommand;
use nullradix_api::{CreateProject, ProjectDto, RenameProject};
use nullradix_client::Client;

#[derive(Subcommand)]
pub(crate) enum ProjectCommand {
    /// List all projects
    List {
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Create a project
    Create {
        /// Key such as ARC, used in ticket keys like ARC-0042
        key: String,
        /// Display name
        name: String,
    },
    /// Rename a project
    Rename { key: String, name: String },
    /// Delete a project
    Delete { key: String },
}

pub(crate) fn run(command: ProjectCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        ProjectCommand::List { json } => {
            let projects = client.list_projects()?.items;
            if json {
                println!("{}", serde_json::to_string_pretty(&projects)?);
            } else if projects.is_empty() {
                println!("no projects yet, create one with `nullradix project create KEY NAME`");
            } else {
                for project in &projects {
                    println!("{:<6}  {}", project.key, project.name);
                }
            }
        }
        ProjectCommand::Create { key, name } => {
            let project = client.create_project(&CreateProject { key, name })?;
            println!("created {} {}", project.key, project.name);
        }
        ProjectCommand::Rename { key, name } => {
            let project = find(client, &key)?;
            let renamed = client.rename_project(
                project.id,
                &RenameProject {
                    name,
                    version: project.version,
                },
            )?;
            println!("renamed {} to {}", renamed.key, renamed.name);
        }
        ProjectCommand::Delete { key } => {
            let project = find(client, &key)?;
            client
                .delete_project(project.id, project.version)
                .with_context(|| format!("cannot delete {key}"))?;
            println!("deleted {key}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn find(client: &Client, key: &str) -> anyhow::Result<ProjectDto> {
    let projects = client.list_projects()?.items;
    match projects.into_iter().find(|project| project.key == key) {
        Some(project) => Ok(project),
        None => bail!("no project has the key {key}"),
    }
}
