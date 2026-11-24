//! The `nullradix` command line and daemon.

mod config;
mod daemon;
mod logging;
mod open;
mod status;
mod token;

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
    /// Show whether the daemon is running
    Status,
    /// Open the web UI in the browser
    Open,
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
    /// Built web UI to serve, such as `web/dist`
    #[arg(long, env = "NULLRADIX_WEB_DIR")]
    web_dir: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    let profile = cli.profile.unwrap_or_else(Profile::of_build);
    match cli.command {
        Command::Serve(args) => {
            let overrides = Overrides {
                bind: args.bind,
                port: args.port,
                log: args.log,
                web_dir: args.web_dir,
            };
            let config = Config::load(profile, cli.home.as_deref(), overrides)?;
            daemon::run(&config)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Status => status::run(profile, cli.home.as_deref()),
        Command::Open => open::run(profile, cli.home.as_deref()),
    }
}
