use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use anyhow::Context;
use clap::Subcommand;
use nullradix_api::{OutputLineDto, OutputStreamDto, RunDto, RunStatusDto, StartRun};
use nullradix_client::Client;
use uuid::Uuid;

const POLL: Duration = Duration::from_millis(200);
/// Labels longer than this are cut; the daemon refuses longer ones.
const LABEL_CHARS: usize = 200;

#[derive(Subcommand)]
pub(crate) enum RunCommand {
    /// Start a command on the daemon and follow its output
    Start {
        /// Name shown in lists, defaults to the command line
        #[arg(long)]
        label: Option<String>,
        /// Working directory, defaults to the current one
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Stop the run after this many seconds
        #[arg(long)]
        timeout: Option<u64>,
        /// Print the run id and return instead of following
        #[arg(long)]
        detach: bool,
        /// Program and its arguments
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// List recent runs, newest first
    List {
        /// At most this many runs
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Print the output of a run
    Logs {
        id: Uuid,
        /// Keep printing until the run finishes
        #[arg(long, short)]
        follow: bool,
    },
    /// Ask a run to stop
    Cancel { id: Uuid },
}

pub(crate) fn run(command: RunCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        RunCommand::Start {
            label,
            cwd,
            timeout,
            detach,
            command,
        } => {
            let cwd = match cwd {
                Some(cwd) => std::path::absolute(&cwd)
                    .with_context(|| format!("cannot resolve {}", cwd.display()))?,
                None => std::env::current_dir().context("cannot read the current directory")?,
            };
            let (program, args) = command.split_first().context("the command is empty")?;
            let input = StartRun {
                label: label.unwrap_or_else(|| default_label(&command)),
                program: program.clone(),
                args: args.to_vec(),
                cwd: cwd.to_string_lossy().into_owned(),
                timeout_seconds: timeout,
            };
            let run = client.start_run(&input)?;
            if detach {
                println!("{}", run.id);
                return Ok(ExitCode::SUCCESS);
            }
            follow(client, run.id)
        }
        RunCommand::List { limit, json } => {
            let runs = client.list_runs(limit)?.items;
            if json {
                println!("{}", serde_json::to_string_pretty(&runs)?);
            } else if runs.is_empty() {
                println!("no runs yet, start one with `nullradix run start -- PROGRAM`");
            } else {
                for run in &runs {
                    println!("{}  {:<10}  {}", run.id, status_name(run.status), run.label);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        RunCommand::Logs { id, follow: true } => follow(client, id),
        RunCommand::Logs { id, follow: false } => {
            let mut after = 0;
            loop {
                let output = client.run_output(id, after)?;
                let Some(last) = output.lines.last() else {
                    break;
                };
                after = last.seq;
                print_lines(&output.lines)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        RunCommand::Cancel { id } => {
            client.cancel_run(id)?;
            println!("asked run {id} to stop");
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Prints output until the run ends and exits like the run did.
fn follow(client: &Client, id: Uuid) -> anyhow::Result<ExitCode> {
    let mut after = 0;
    loop {
        let output = client.run_output(id, after)?;
        if let Some(last) = output.lines.last() {
            after = last.seq;
            print_lines(&output.lines)?;
        } else if output.finished {
            break;
        } else {
            thread::sleep(POLL);
        }
    }
    // the daemon records the end shortly after the runner reports it
    let run = loop {
        let run = client.get_run(id)?;
        if !matches!(run.status, RunStatusDto::Starting | RunStatusDto::Running) {
            break run;
        }
        thread::sleep(POLL);
    };
    Ok(exit_code(&run))
}

fn print_lines(lines: &[OutputLineDto]) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    for line in lines {
        match line.stream {
            OutputStreamDto::Stdout => writeln!(stdout, "{}", line.text)?,
            OutputStreamDto::Stderr => writeln!(stderr, "{}", line.text)?,
        }
    }
    stdout.flush()?;
    stderr.flush()
}

fn exit_code(run: &RunDto) -> ExitCode {
    match run.status {
        RunStatusDto::Succeeded => ExitCode::SUCCESS,
        RunStatusDto::Failed => {
            if let Some(error) = &run.error {
                eprintln!("error: {error}");
            }
            run.exit_code
                .and_then(|code| u8::try_from(code).ok())
                .filter(|code| *code != 0)
                .map_or(ExitCode::FAILURE, ExitCode::from)
        }
        status => {
            eprintln!("run {}: {}", run.id, status_name(status));
            ExitCode::FAILURE
        }
    }
}

const fn status_name(status: RunStatusDto) -> &'static str {
    match status {
        RunStatusDto::Starting => "starting",
        RunStatusDto::Running => "running",
        RunStatusDto::Succeeded => "succeeded",
        RunStatusDto::Failed => "failed",
        RunStatusDto::Cancelled => "cancelled",
        RunStatusDto::TimedOut => "timed-out",
        RunStatusDto::Lost => "lost",
    }
}

fn default_label(command: &[String]) -> String {
    command.join(" ").chars().take(LABEL_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_default_to_the_command_line_within_the_limit() {
        let short = vec![String::from("cargo"), String::from("test")];
        assert_eq!(default_label(&short), "cargo test");
        let long = vec!["x".repeat(500)];
        assert_eq!(default_label(&long).chars().count(), LABEL_CHARS);
    }
}
