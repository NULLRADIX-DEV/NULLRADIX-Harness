//! The `nullradix` command line and daemon.

mod account;
mod board;
mod claude_slots;
mod comment;
mod config;
mod connect;
mod customize;
mod daemon;
mod dispatcher;
mod dispatcher_prompt;
mod hook;
mod inbox;
mod intake;
mod logging;
mod note;
mod open;
mod plan;
mod project;
mod question;
mod run;
mod secrets;
mod selection;
mod session;
mod status;
mod step;
mod theme;
mod theme_catalog;
mod themes;
mod ticket;
mod token;

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};

use nullradix_os::Paths;

use crate::config::{Config, Overrides, Profile};
use crate::selection::BoardChoice;

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
    /// Manage the Claude accounts runs are spread over
    #[command(subcommand)]
    Account(account::AccountCommand),
    /// Configure the dispatcher that starts ready tickets
    #[command(subcommand)]
    Dispatcher(dispatcher::DispatcherCommand),
    /// Manage projects
    #[command(subcommand)]
    Project(project::ProjectCommand),
    /// Manage the boards of projects
    #[command(subcommand)]
    Board(board::BoardCommand),
    /// Create, list and show tickets
    #[command(subcommand)]
    Ticket(ticket::TicketCommand),
    /// Ask a person, answer and comment on questions
    #[command(subcommand)]
    Question(question::QuestionCommand),
    /// Check, add, edit, remove and move the work steps of a ticket
    #[command(subcommand)]
    Step(step::StepCommand),
    /// Write plans and claim tickets for planning
    #[command(subcommand)]
    Plan(plan::PlanCommand),
    /// Keep findings for later tickets: add, count, list, promote and drop notes
    #[command(subcommand)]
    Note(note::NoteCommand),
    /// What waits for you on the boards: failed merges, questions, acceptances, notes
    #[command(after_help = inbox::INBOX_AFTER)]
    Inbox(inbox::InboxArgs),
    /// List and show Claude Code sessions
    #[command(subcommand)]
    Session(session::SessionCommand),
    /// Replace or add skills and output styles: show, copy, set, switch and reset
    #[command(subcommand)]
    Customize(customize::CustomizeCommand),
    /// List, install, choose, remove and pack themes
    #[command(subcommand)]
    Theme(theme::ThemeCommand),
    /// Run commands on the daemon's machine
    #[command(subcommand)]
    Run(run::RunCommand),
    /// Supervise one run; the daemon starts this, not people
    #[command(hide = true)]
    RunSupervisor { dir: PathBuf },
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
    /// Let theme catalogs use http and local or private hosts
    #[arg(long, env = "NULLRADIX_THEME_CATALOG_LOCAL")]
    theme_catalog_local: bool,
}

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if let Some(call) = hook::HookCall::parse(&args) {
        return hook::run_plain(call);
    }
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn choice(profile: Profile, home: Option<&Path>) -> anyhow::Result<BoardChoice> {
    Ok(BoardChoice::in_dir(
        &Paths::resolve(profile.app_name(), home)?.config,
    ))
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
                theme_catalog_local: args.theme_catalog_local,
            };
            let config = Config::load(profile, cli.home.as_deref(), overrides)?;
            daemon::run(&config)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Status => status::run(profile, cli.home.as_deref()),
        Command::Open => open::run(profile, cli.home.as_deref()),
        Command::Account(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            account::run(command, &client)
        }
        Command::Dispatcher(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            dispatcher::run(command, &client)
        }
        Command::Project(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            project::run(command, &client)
        }
        Command::Board(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            board::run(command, &client, &choice(profile, cli.home.as_deref())?)
        }
        Command::Ticket(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            ticket::run(command, &client, &choice(profile, cli.home.as_deref())?)
        }
        Command::Note(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            note::run(command, &client, &choice(profile, cli.home.as_deref())?)
        }
        Command::Question(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            question::run(command, &client, &choice(profile, cli.home.as_deref())?)
        }
        Command::Step(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            step::run(command, &client)
        }
        Command::Plan(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            plan::run(command, &client, &choice(profile, cli.home.as_deref())?)
        }
        Command::Inbox(args) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            inbox::run(&args, &client)
        }
        Command::Session(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            session::run(command, &client)
        }
        Command::Customize(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            customize::run(command, &client)
        }
        Command::Theme(theme::ThemeCommand::Pack { dir, out }) => theme::pack(&dir, out.as_deref()),
        Command::Theme(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            theme::run(command, &client)
        }
        Command::Run(command) => {
            let client = connect::connect(profile, cli.home.as_deref())?;
            run::run(command, &client)
        }
        Command::RunSupervisor { dir } => {
            nullradix_runner::supervise(&dir)
                .with_context(|| format!("cannot record the result in {}", dir.display()))?;
            Ok(ExitCode::SUCCESS)
        }
    }
}
