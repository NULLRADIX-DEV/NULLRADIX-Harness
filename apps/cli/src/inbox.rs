use std::process::ExitCode;

use nullradix_api::{InboxItemDto, InboxItemTypeDto, InboxKindDto};
use nullradix_client::Client;

use crate::selection::BoardOption;

pub(crate) const INBOX_AFTER: &str = "Examples:
  nullradix inbox --md
  nullradix inbox --kind merge,accept";

#[derive(clap::Args)]
pub(crate) struct InboxArgs {
    /// Only this board; all boards when left out
    #[arg(long)]
    board: Option<BoardOption>,
    /// Only these kinds, split by commas: merge, setup, question, accept, blocked, planning, note
    #[arg(long)]
    kind: Option<String>,
    /// Print Markdown
    #[arg(long, conflicts_with = "json")]
    md: bool,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: &InboxArgs, client: &Client) -> anyhow::Result<ExitCode> {
    let board = match args.board {
        Some(BoardOption::One(board)) => Some(board),
        Some(BoardOption::All) | None => None,
    };
    let items = client.get_inbox(board, args.kind.as_deref())?.items;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(ExitCode::SUCCESS);
    }
    let lines = if args.md {
        markdown(&items)
    } else {
        text(&items)
    };
    for line in lines {
        println!("{line}");
    }
    Ok(ExitCode::SUCCESS)
}

const fn label(kind: InboxKindDto) -> &'static str {
    match kind {
        InboxKindDto::Merge => "Merge failed",
        InboxKindDto::Setup => "Setup failed",
        InboxKindDto::Question => "Question",
        InboxKindDto::Accept => "Acceptance",
        InboxKindDto::Blocked => "Blocked",
        InboxKindDto::Planning => "In planning",
        InboxKindDto::Note => "Note",
    }
}

/// The key column: ticket or question key, or `Note 7`.
fn id(item: &InboxItemDto) -> String {
    match item.item_type {
        InboxItemTypeDto::Note => format!("Note {}", item.key),
        InboxItemTypeDto::Ticket | InboxItemTypeDto::Question => item.key.clone(),
    }
}

/// Project, board and, for questions, how many tickets wait.
fn place(item: &InboxItemDto) -> String {
    let board = item
        .board_name
        .as_ref()
        .map_or_else(String::new, |name| format!(" / {name}"));
    let waiting = if item.kind == InboxKindDto::Question {
        format!(", {} ticket(s) waiting", item.waiting_tickets)
    } else {
        String::new()
    };
    format!("{}{board}{waiting}", item.project_key)
}

/// The command that moves the entry on.
fn action(item: &InboxItemDto) -> String {
    match item.kind {
        InboxKindDto::Accept => format!("nullradix ticket accept {}", item.key),
        InboxKindDto::Question => format!("nullradix question show {}", item.key),
        InboxKindDto::Note => format!("nullradix note promote {}", item.key),
        _ => format!("nullradix ticket show {}", item.key),
    }
}

fn text(items: &[InboxItemDto]) -> Vec<String> {
    if items.is_empty() {
        return vec![String::from("Nothing waits for you.")];
    }
    let mut lines = Vec::new();
    for (index, item) in items.iter().enumerate() {
        lines.push(format!(
            "{}. [{}] {}  {}  ({})",
            index + 1,
            label(item.kind),
            id(item),
            item.title,
            place(item)
        ));
        if let Some(detail) = &item.detail {
            lines.push(format!("   {detail}"));
        }
        lines.push(format!("   -> {}", action(item)));
    }
    lines
}

fn markdown(items: &[InboxItemDto]) -> Vec<String> {
    if items.is_empty() {
        return vec![String::from("_Nothing waits for you._")];
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let detail = item.detail.as_ref().map_or_else(String::new, |detail| {
                format!(
                    " - {}",
                    detail.split_whitespace().collect::<Vec<_>>().join(" ")
                )
            });
            format!(
                "{}. **{}** `{}` {} ({}){detail} -> `{}`",
                index + 1,
                label(item.kind),
                id(item),
                item.title,
                place(item),
                action(item)
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(item_type: InboxItemTypeDto, key: &str, kind: InboxKindDto) -> InboxItemDto {
        InboxItemDto {
            item_type,
            key: key.to_owned(),
            kind,
            title: String::from("Title"),
            detail: Some(String::from("why\nit waits")),
            board_id: None,
            board_name: Some(String::from("Main")),
            project_key: String::from("ARC"),
            waiting_tickets: 2,
            priority: None,
            since_at: 0,
            rank: 1,
        }
    }

    #[test]
    fn empty_inboxes_say_so() {
        assert_eq!(text(&[]), ["Nothing waits for you."]);
        assert_eq!(markdown(&[]), ["_Nothing waits for you._"]);
    }

    #[test]
    fn entries_name_their_place_and_next_command() {
        let question = item(
            InboxItemTypeDto::Question,
            "ARC-Q0001",
            InboxKindDto::Question,
        );
        assert_eq!(
            text(std::slice::from_ref(&question)),
            [
                "1. [Question] ARC-Q0001  Title  (ARC / Main, 2 ticket(s) waiting)",
                "   why\nit waits",
                "   -> nullradix question show ARC-Q0001",
            ]
        );
        let note = item(InboxItemTypeDto::Note, "7", InboxKindDto::Note);
        assert_eq!(
            markdown(&[note]),
            [
                "1. **Note** `Note 7` Title (ARC / Main) - why it waits -> `nullradix note promote 7`"
            ]
        );
        let accept = item(InboxItemTypeDto::Ticket, "ARC-0001", InboxKindDto::Accept);
        assert_eq!(action(&accept), "nullradix ticket accept ARC-0001");
        let merge = item(InboxItemTypeDto::Ticket, "ARC-0001", InboxKindDto::Merge);
        assert_eq!(action(&merge), "nullradix ticket show ARC-0001");
    }
}
