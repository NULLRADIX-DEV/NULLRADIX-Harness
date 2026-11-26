use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod checks;
mod codegen;
mod db;
mod files;
mod gate;

#[derive(Parser)]
#[command(name = "xtask", about = "Repository tasks for NULLRADIX")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the repository checks
    Check,
    /// Manage the offline query cache of the store
    #[command(subcommand)]
    Db(Db),
    /// Write the OpenAPI document for the web client
    Openapi,
    /// Write the OpenAPI document and regenerate the web API types
    Codegen,
    /// Run every check CI runs and print one summary
    Gate {
        /// Skip lint, typecheck, tests and build of the web app
        #[arg(long)]
        skip_web: bool,
    },
}

#[derive(Subcommand)]
enum Db {
    /// Regenerate the query cache from the migrations
    Prepare,
    /// Fail when the query cache is out of date
    Check,
}

fn main() -> ExitCode {
    match run(&Cli::parse().command) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: &Command) -> Result<ExitCode> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask must live inside the workspace")?;
    match command {
        Command::Check => check(root),
        Command::Db(Db::Prepare) => db::prepare(root),
        Command::Db(Db::Check) => db::check(root),
        Command::Openapi => {
            codegen::openapi(root)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Codegen => {
            codegen::codegen(root)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Gate { skip_web } => Ok(gate::gate(root, *skip_web)),
    }
}

fn check(root: &Path) -> Result<ExitCode> {
    let violations = checks::run(root)?;
    if violations.is_empty() {
        println!("checks passed");
        return Ok(ExitCode::SUCCESS);
    }
    for violation in &violations {
        println!("{violation}");
    }
    println!("{} violation(s)", violations.len());
    Ok(ExitCode::FAILURE)
}
