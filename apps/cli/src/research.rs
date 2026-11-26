use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context;
use clap::Subcommand;
use nullradix_api::SubmitResearch;
use nullradix_client::Client;

use crate::comment::agent_session;

/// Largest research file read; the daemon limits the parts on top.
pub(crate) const FILE_MAX_BYTES: u64 = 1024 * 1024;

const FILE_FORMAT: &str = r#"File: {"context": "...", "options": [{"title": "...", "text": "...", "pros": ["..."], "cons": ["..."], "recommended": true}]}
Example:
  nullradix research submit ARC-Q7 --from research.json --author research-ARC-Q7"#;

#[derive(Subcommand)]
pub(crate) enum ResearchCommand {
    /// Save a research on a question: a context, up to five options, or both
    #[command(after_help = FILE_FORMAT)]
    Submit(SubmitArgs),
    /// Start a research run on an open question
    Start {
        /// Question key such as ARC-Q7
        key: String,
    },
}

#[derive(clap::Args)]
pub(crate) struct SubmitArgs {
    /// Question key such as ARC-Q7
    key: String,
    /// JSON file with the research; `-` reads standard input
    #[arg(long, alias = "file", conflicts_with = "context")]
    from: Option<PathBuf>,
    /// A research of context text only
    #[arg(long, required_unless_present = "from")]
    context: Option<String>,
    /// Who researched, such as research-ARC-Q7; the person the daemon acts for when left out
    #[arg(long)]
    author: Option<String>,
}

/// Reads at most [`FILE_MAX_BYTES`] of a file or of standard input; `what` names it in the error.
pub(crate) fn read_limited(path: &Path, what: &str) -> anyhow::Result<String> {
    let mut text = String::new();
    let read = if path == Path::new("-") {
        std::io::stdin()
            .take(FILE_MAX_BYTES + 1)
            .read_to_string(&mut text)
            .context("cannot read standard input")?
    } else {
        std::fs::File::open(path)
            .with_context(|| format!("cannot read {}", path.display()))?
            .take(FILE_MAX_BYTES + 1)
            .read_to_string(&mut text)
            .with_context(|| format!("cannot read {}", path.display()))?
    };
    anyhow::ensure!(
        read as u64 <= FILE_MAX_BYTES,
        "the {what} has more than {FILE_MAX_BYTES} bytes"
    );
    Ok(text)
}

/// The research as typed; the author and session of the call replace those in a file.
fn input(args: &SubmitArgs, session: Option<String>) -> anyhow::Result<SubmitResearch> {
    let mut research = match (&args.from, &args.context) {
        (Some(path), _) => serde_json::from_str::<SubmitResearch>(&read_limited(path, "research")?)
            .context("the research is not valid JSON of the expected shape")?,
        (None, context) => SubmitResearch {
            context: context.clone(),
            ..SubmitResearch::default()
        },
    };
    research.author.clone_from(&args.author);
    research.session = session;
    Ok(research)
}

pub(crate) fn run(command: ResearchCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        ResearchCommand::Submit(args) => {
            let detail =
                client.submit_research(args.key.trim(), &input(&args, agent_session())?)?;
            let options = detail
                .research
                .last()
                .map_or(0, |round| round.options.len());
            println!(
                "Research saved on {}: {options} option(s).",
                detail.question.key
            );
        }
        ResearchCommand::Start { key } => {
            let run = client.start_research_run(key.trim())?;
            println!("Research started: run {}.", run.id);
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(from: Option<&Path>, context: Option<&str>) -> SubmitArgs {
        SubmitArgs {
            key: String::from("ARC-Q7"),
            from: from.map(Path::to_path_buf),
            context: context.map(str::to_owned),
            author: Some(String::from("research-ARC-Q7")),
        }
    }

    #[test]
    fn a_context_alone_is_a_research() {
        let research = input(&args(None, Some("Today.")), Some(String::from("s-1"))).unwrap();
        assert_eq!(research.context.as_deref(), Some("Today."));
        assert_eq!(research.options.len(), 0);
        assert_eq!(research.author.as_deref(), Some("research-ARC-Q7"));
        assert_eq!(research.session.as_deref(), Some("s-1"));
    }

    #[test]
    fn a_file_gives_context_and_options_and_the_call_names_the_author() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("research.json");
        let json = r#"{"context":"Today.","options":[{"title":"A","text":"x","recommended":true}],"author":"someone"}"#;
        std::fs::write(&file, json).unwrap();
        let research = input(&args(Some(&file), None), None).unwrap();
        assert_eq!(research.options.len(), 1);
        assert!(research.options[0].recommended);
        assert_eq!(research.author.as_deref(), Some("research-ARC-Q7"));
        assert_eq!(research.session, None);
    }

    #[test]
    fn bad_files_are_refused_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "no json").unwrap();
        let error = input(&args(Some(&broken), None), None).unwrap_err();
        assert!(error.to_string().contains("not valid JSON"), "{error}");
        let missing = dir.path().join("missing.json");
        assert!(
            input(&args(Some(&missing), None), None)
                .unwrap_err()
                .to_string()
                .contains("cannot read")
        );
        let unknown = dir.path().join("unknown.json");
        std::fs::write(&unknown, r#"{"surprise":1}"#).unwrap();
        assert!(input(&args(Some(&unknown), None), None).is_err());
    }

    #[test]
    fn a_file_just_above_the_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let exact = dir.path().join("exact.json");
        let pad = " ".repeat(usize::try_from(FILE_MAX_BYTES).unwrap() - 2);
        std::fs::write(&exact, format!("{{{pad}}}")).unwrap();
        assert!(input(&args(Some(&exact), None), None).is_ok());
        let over = dir.path().join("over.json");
        let pad = " ".repeat(usize::try_from(FILE_MAX_BYTES).unwrap() - 1);
        std::fs::write(&over, format!("{{{pad}}}")).unwrap();
        let error = input(&args(Some(&over), None), None).unwrap_err();
        assert!(error.to_string().contains("more than"), "{error}");
    }
}
