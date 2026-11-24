//! The `nullradix` command line and daemon.

mod config;
mod daemon;
mod logging;

use std::net::IpAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::config::{Config, Overrides, Profile};

#[derive(Parser)]
#[command(
    name = "nullradix",
    version,
    about = "Local workbench for parallel Claude Code sessions"
)]
struct Cli {
    /// Data profile, defaults to dev for debug builds
    #[arg(long, env = "NULLRADIX_PROFILE", value_enum, global = true)]
    profile: Option<Profile>,
    /// Keep all data below this directory instead of the platform defaults
    #[arg(long, env = "NULLRADIX_HOME", global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon in the foreground
    Serve(ServeArgs),
}

#[derive(Args)]
struct ServeArgs {
    /// Address to listen on
    #[arg(long, env = "NULLRADIX_BIND")]
    bind: Option<IpAddr>,
    /// Port to listen on
    #[arg(long, env = "NULLRADIX_PORT")]
    port: Option<u16>,
    /// Log filter such as `info` or `nullradix=debug`
    #[arg(long, env = "NULLRADIX_LOG")]
    log: Option<String>,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let profile = cli.profile.unwrap_or_else(Profile::of_build);
    match cli.command {
        Command::Serve(args) => {
            let overrides = Overrides {
                bind: args.bind,
                port: args.port,
                log: args.log,
            };
            let config = Config::load(profile, cli.home.as_deref(), overrides)?;
            daemon::run(&config)
        }
    }
}
