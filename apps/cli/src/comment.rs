use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use nullradix_api::{ActorKindDto, AddComment, CommentDto};
use nullradix_client::Client;
use nullradix_domain::Timestamp;

use crate::ticket::canonical;

/// Agent comments `ticket show` keeps without `--all-comments`; people's always show.
const KEPT_AGENT_COMMENTS: usize = 3;

#[derive(clap::Args)]
pub(crate) struct CommentArgs {
    /// Ticket key such as ARC-7
    key: String,
    /// The comment, such as progress, evidence or a reason
    #[arg(required_unless_present = "body_file", conflicts_with = "body_file")]
    text: Option<String>,
    /// Read the comment from this file; `-` reads standard input
    #[arg(long)]
    body_file: Option<PathBuf>,
    /// Who comments, such as claude; the person the daemon acts for when left out
    #[arg(long)]
    author: Option<String>,
}

fn body(text: Option<String>, file: Option<&Path>) -> anyhow::Result<String> {
    match file {
        Some(path) if path == Path::new("-") => {
            io::read_to_string(io::stdin()).context("cannot read standard input")
        }
        Some(path) => {
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))
        }
        None => Ok(text.unwrap_or_default()),
    }
}

/// The agent session from `NULLRADIX_SESSION`; an empty value counts as none.
fn session(value: Option<String>) -> Option<String> {
    value.filter(|session| !session.trim().is_empty())
}

/// The agent session this CLI runs in, as hooks export it.
pub(crate) fn agent_session() -> Option<String> {
    session(std::env::var("NULLRADIX_SESSION").ok())
}

pub(crate) fn run(args: CommentArgs, client: &Client) -> anyhow::Result<()> {
    let input = AddComment {
        body: body(args.text, args.body_file.as_deref())?,
        author: args.author,
        session: agent_session(),
    };
    let comment = client.add_comment(&canonical(&args.key)?, &input)?;
    println!("Comment on {} saved.", comment.ticket_key);
    Ok(())
}

/// Comments oldest first; without `all` only the last agent comments stay, as in legacy.
pub(crate) fn comment_lines(comments: &[CommentDto], all: bool) -> Vec<String> {
    let by_agents: Vec<usize> = comments
        .iter()
        .enumerate()
        .filter(|(_, comment)| comment.author_kind != ActorKindDto::Human)
        .map(|(index, _)| index)
        .collect();
    let hidden = if all {
        &[][..]
    } else {
        &by_agents[..by_agents.len().saturating_sub(KEPT_AGENT_COMMENTS)]
    };
    let mut lines = Vec::new();
    if !hidden.is_empty() {
        let noun = if hidden.len() == 1 {
            "comment"
        } else {
            "comments"
        };
        lines.push(format!(
            "({} older agent {noun} hidden; --all-comments shows them)",
            hidden.len()
        ));
    }
    for (index, comment) in comments.iter().enumerate() {
        if hidden.contains(&index) {
            continue;
        }
        let mut body = comment.body.lines();
        let agent = if comment.author_kind == ActorKindDto::Human {
            ""
        } else {
            " (agent)"
        };
        lines.push(format!(
            "{}  {}{agent}: {}",
            Timestamp::from_millis(comment.created_at),
            comment.author_name,
            body.next().unwrap_or_default()
        ));
        lines.extend(body.map(|line| format!("    {line}")));
    }
    lines
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use uuid::Uuid;

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: CommentArgs,
    }

    #[test]
    fn an_empty_session_counts_as_none() {
        assert_eq!(session(None), None);
        assert_eq!(session(Some(String::from(" "))), None);
        assert_eq!(session(Some(String::from("s-1"))).as_deref(), Some("s-1"));
    }

    #[test]
    fn takes_text_or_a_body_file_but_not_both() {
        let parse = |args: &[&str]| Cli::try_parse_from([&["comment", "ARC-1"], args].concat());
        let parsed = parse(&["Step 2 done", "--author", "claude"]).unwrap().args;
        assert_eq!(parsed.text.as_deref(), Some("Step 2 done"));
        assert_eq!(parsed.author.as_deref(), Some("claude"));
        assert!(parse(&["--body-file", "-"]).is_ok());
        let both = parse(&["text", "--body-file", "note.md"]).err().unwrap();
        assert_eq!(both.kind(), clap::error::ErrorKind::ArgumentConflict);
        let none = parse(&[]).err().unwrap();
        assert_eq!(none.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    fn comment(author: &str, kind: ActorKindDto, body: &str) -> CommentDto {
        CommentDto {
            id: Uuid::nil(),
            ticket_key: String::from("ARC-0001"),
            author_id: Uuid::nil(),
            author_name: author.to_owned(),
            author_kind: kind,
            body: body.to_owned(),
            system_marker: None,
            created_at: 0,
        }
    }

    #[test]
    fn keeps_the_last_three_agent_comments_and_every_person_one() {
        let mut comments = vec![comment("anna", ActorKindDto::Human, "first\nsecond line")];
        for number in 1..=4 {
            comments.push(comment(
                "claude",
                ActorKindDto::Session,
                &number.to_string(),
            ));
        }
        let short = comment_lines(&comments, false);
        assert_eq!(
            short,
            [
                "(1 older agent comment hidden; --all-comments shows them)",
                "1970-01-01T00:00:00.000Z  anna: first",
                "    second line",
                "1970-01-01T00:00:00.000Z  claude (agent): 2",
                "1970-01-01T00:00:00.000Z  claude (agent): 3",
                "1970-01-01T00:00:00.000Z  claude (agent): 4",
            ]
        );
        assert_eq!(comment_lines(&comments, true).len(), 6);
    }
}
