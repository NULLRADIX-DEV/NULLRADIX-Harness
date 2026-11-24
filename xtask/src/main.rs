use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod checks;
mod files;

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
