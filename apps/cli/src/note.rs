use std::process::ExitCode;

use clap::Subcommand;
use nullradix_api::{CreateNote, DropNote, NoteDto, NoteStateDto, PromoteNote, SeenNote};
use nullradix_client::Client;
use nullradix_domain::Timestamp;
use uuid::Uuid;

use crate::selection::BoardChoice;
use crate::ticket::canonical;

const ADD_AFTER: &str = "The title is one sentence that makes sense without context: what is \
broken or missing, without a ticket key. Where it came from goes into --from.

Example:
  nullradix note add \"Cache of the board view never expires\" --details \"BoardView keeps \
the old column after a ticket change; check the change marker.\" --from ARC-9 --label cache \
--author claude";

#[derive(Subcommand)]
pub(crate) enum NoteCommand {
    /// Keep a finding that does not belong to the current ticket; a known one is refused
    #[command(after_help = ADD_AFTER)]
    Add(AddArgs),
    /// The same finding stood out again: count the open note instead of adding one
    #[command(after_help = "Example:\n  nullradix note seen 12 --from ARC-14")]
    Seen {
        /// Note number such as 12
        number: String,
        /// Ticket it stood out at again
        #[arg(long)]
        from: Option<String>,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// List notes by number; only open ones without --state
    #[command(
        after_help = "Examples:\n  nullradix note list --md\n  nullradix note list --state all"
    )]
    List(ListArgs),
    /// Show a note in full
    Show {
        /// Note number such as 12
        number: String,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Make a ticket from an open note: its title, details as description, labels go along
    #[command(after_help = "Example:\n  nullradix note promote 12 --priority high")]
    Promote(PromoteArgs),
    /// Drop a note with a reason; notes are never deleted
    #[command(after_help = "Example:\n  nullradix note drop 12 --reason \"settled by ARC-20\"")]
    Drop {
        /// Note number such as 12
        number: String,
        /// Why nothing comes of it (required)
        #[arg(long)]
        reason: Option<String>,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args)]
pub(crate) struct AddArgs {
    title: String,
    /// Where, how to see it, suspected cause, proposal
    #[arg(long)]
    details: Option<String>,
    /// Ticket it stood out at
    #[arg(long)]
    from: Option<String>,
    /// A label; repeat the flag for more
    #[arg(long = "label")]
    labels: Vec<String>,
    /// Who notes it, such as claude
    #[arg(long)]
    author: Option<String>,
    /// Board; by default the --from ticket's, else the repository's, else the active one
    #[arg(long)]
    board: Option<Uuid>,
    /// Add it even when a similar note exists
    #[arg(long)]
    force: bool,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
pub(crate) struct ListArgs {
    /// open, promoted, dropped or all
    #[arg(long)]
    state: Option<String>,
    /// Only notes found at this ticket
    #[arg(long)]
    from: Option<String>,
    /// Print Markdown
    #[arg(long, conflicts_with = "json")]
    md: bool,
    /// Board to list; by default the repository's board, else the active one
    #[arg(long)]
    board: Option<Uuid>,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
pub(crate) struct PromoteArgs {
    /// Note number such as 12
    number: String,
    /// Another title for the ticket
    #[arg(long)]
    title: Option<String>,
    /// Status to start in, backlog when left out
    #[arg(long)]
    status: Option<String>,
    /// low, normal, high or urgent
    #[arg(long)]
    priority: Option<String>,
    /// Replaces the labels; repeat the flag for more
    #[arg(long = "label")]
    labels: Vec<String>,
    /// Who promotes it
    #[arg(long)]
    by: Option<String>,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(
    command: NoteCommand,
    client: &Client,
    choice: &BoardChoice,
) -> anyhow::Result<ExitCode> {
    match command {
        NoteCommand::Add(args) => add(args, client, choice)?,
        NoteCommand::Seen { number, from, json } => {
            let note = client.seen_note(&number, &SeenNote { source: from })?;
            print(
                &note,
                json,
                &[format!(
                    "Note {}: seen {} times.",
                    note.number, note.seen_count
                )],
            )?;
        }
        NoteCommand::List(args) => list(&args, client, choice)?,
        NoteCommand::Show { number, json } => {
            let note = client.get_note(&number)?;
            print(&note, json, &show_lines(&note))?;
        }
        NoteCommand::Promote(args) => {
            let input = PromoteNote {
                title: args.title,
                status: args.status,
                priority: args.priority,
                labels: (!args.labels.is_empty()).then_some(args.labels),
                by: args.by,
            };
            let promoted = client.promote_note(&args.number, &input)?;
            print(
                &promoted,
                args.json,
                &[format!(
                    "Note {} -> {} {}",
                    promoted.note.number, promoted.ticket.key, promoted.ticket.title
                )],
            )?;
        }
        NoteCommand::Drop {
            number,
            reason,
            json,
        } => {
            let input = DropNote {
                reason: reason.unwrap_or_default(),
            };
            let note = client.drop_note(&number, &input)?;
            print(&note, json, &[format!("Note {} dropped.", note.number)])?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print<T: serde::Serialize>(value: &T, json: bool, lines: &[String]) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        for line in lines {
            println!("{line}");
        }
    }
    Ok(())
}

fn add(args: AddArgs, client: &Client, choice: &BoardChoice) -> anyhow::Result<()> {
    // without --board the board of the ticket it came from counts
    let (project, board) = match (args.board, &args.from) {
        (None, Some(from)) => {
            let ticket = client.get_ticket(&canonical(from)?)?.ticket;
            (ticket.project_id, ticket.board_id)
        }
        (board, _) => {
            let board = choice.board(client, board)?;
            (board.project_id, board.id)
        }
    };
    let input = CreateNote {
        title: args.title,
        body: args.details,
        source: args.from,
        labels: args.labels,
        author: args.author,
        force: args.force,
    };
    let note = client.add_note(project, board, &input)?;
    print(&note, args.json, &[format!("Note {} saved.", note.number)])
}

fn list(args: &ListArgs, client: &Client, choice: &BoardChoice) -> anyhow::Result<()> {
    let board = choice.board(client, args.board)?;
    let mut query = Vec::new();
    if let Some(state) = &args.state {
        query.push(("state", state.as_str()));
    }
    if let Some(from) = &args.from {
        query.push(("source", from.as_str()));
    }
    let notes = client.list_notes(board.project_id, board.id, &query)?.items;
    let lines = if args.md {
        markdown_lines(&notes)
    } else {
        list_lines(&notes)
    };
    print(&notes, args.json, &lines)
}

const fn state_name(state: NoteStateDto) -> &'static str {
    match state {
        NoteStateDto::Open => "open",
        NoteStateDto::Promoted => "promoted",
        NoteStateDto::Dropped => "dropped",
    }
}

/// Title to show; notes without one show the first line of their body.
fn title(note: &NoteDto) -> String {
    let title = note.title.trim();
    if !title.is_empty() {
        return title.to_owned();
    }
    note.body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(|| format!("Note {}", note.number), str::to_owned)
}

/// One line per note, such as `  12  [open] Cache never expires [3x] (from ARC-0009)  cache`.
fn list_lines(notes: &[NoteDto]) -> Vec<String> {
    if notes.is_empty() {
        return vec![String::from("(no notes)")];
    }
    notes
        .iter()
        .map(|note| {
            let mut parts = vec![format!(
                "  {}  [{}] {}",
                note.number,
                state_name(note.state),
                title(note)
            )];
            if note.seen_count > 1 {
                parts.push(format!(" [{}x]", note.seen_count));
            }
            if let Some(source) = &note.source {
                parts.push(format!(" (from {source})"));
            }
            if !note.labels.is_empty() {
                parts.push(format!("  {}", note.labels.join(",")));
            }
            parts.concat()
        })
        .collect()
}

fn markdown_lines(notes: &[NoteDto]) -> Vec<String> {
    if notes.is_empty() {
        return vec![String::from("- (no notes)")];
    }
    notes
        .iter()
        .map(|note| {
            let mut meta = vec![state_name(note.state).to_owned()];
            if note.seen_count > 1 {
                meta.push(format!("seen {} times", note.seen_count));
            }
            if let Some(source) = &note.source {
                meta.push(format!("from {source}"));
            }
            if let Some(ticket) = &note.ticket_key {
                meta.push(format!("-> {ticket}"));
            }
            if !note.labels.is_empty() {
                meta.push(note.labels.join(","));
            }
            format!(
                "- **{}** {} - {}",
                note.number,
                title(note),
                meta.join(" | ")
            )
        })
        .collect()
}

fn show_lines(note: &NoteDto) -> Vec<String> {
    let mut lines = vec![format!(
        "Note {}  [{}]  {}  {}",
        note.number,
        state_name(note.state),
        Timestamp::from_millis(note.created_at),
        note.author_name
    )];
    if let Some(source) = &note.source {
        lines.push(format!("From: {source}"));
    }
    if note.seen_count > 1 {
        let also = if note.seen_in.is_empty() {
            String::new()
        } else {
            format!(" (also at {})", note.seen_in.join(", "))
        };
        lines.push(format!("Seen: {} times{also}", note.seen_count));
    }
    if !note.labels.is_empty() {
        lines.push(format!("Labels: {}", note.labels.join(", ")));
    }
    if let Some(ticket) = &note.ticket_key {
        lines.push(format!("Ticket: {ticket}"));
    }
    if let Some(reason) = &note.reason {
        lines.push(format!("Reason: {reason}"));
    }
    if !note.title.is_empty() {
        lines.push(String::new());
        lines.push(format!("# {}", note.title));
    }
    if !note.body.trim().is_empty() {
        lines.push(String::new());
        lines.push(note.body.trim_end().to_owned());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(number: i64) -> NoteDto {
        NoteDto {
            number,
            id: Uuid::from_u128(1),
            board_id: Uuid::from_u128(2),
            author_id: Uuid::from_u128(3),
            author_name: String::from("claude"),
            title: String::from("Cache never expires"),
            body: String::new(),
            source: None,
            labels: Vec::new(),
            seen_count: 1,
            seen_in: Vec::new(),
            state: NoteStateDto::Open,
            ticket_key: None,
            reason: None,
            created_at: 0,
            updated_at: 0,
            version: 1,
        }
    }

    #[test]
    fn lists_notes_as_text_and_markdown() {
        let plain = note(1);
        let busy = NoteDto {
            seen_count: 3,
            source: Some(String::from("ARC-0009")),
            labels: vec![String::from("cache"), String::from("ui")],
            state: NoteStateDto::Promoted,
            ticket_key: Some(String::from("ARC-0011")),
            ..note(2)
        };
        assert_eq!(
            list_lines(&[plain.clone(), busy.clone()]),
            [
                "  1  [open] Cache never expires",
                "  2  [promoted] Cache never expires [3x] (from ARC-0009)  cache,ui",
            ]
        );
        assert_eq!(
            markdown_lines(&[plain, busy]),
            [
                "- **1** Cache never expires - open",
                "- **2** Cache never expires - promoted | seen 3 times | from ARC-0009 | -> \
                 ARC-0011 | cache,ui",
            ]
        );
        assert_eq!(list_lines(&[]), ["(no notes)"]);
        assert_eq!(markdown_lines(&[]), ["- (no notes)"]);
    }

    #[test]
    fn shows_a_note_in_full() {
        let full = NoteDto {
            body: String::from("Details here.\n"),
            seen_count: 2,
            seen_in: vec![String::from("ARC-0003")],
            source: Some(String::from("ARC-0001")),
            labels: vec![String::from("cache")],
            state: NoteStateDto::Dropped,
            reason: Some(String::from("fixed")),
            ..note(4)
        };
        assert_eq!(
            show_lines(&full),
            [
                "Note 4  [dropped]  1970-01-01T00:00:00.000Z  claude",
                "From: ARC-0001",
                "Seen: 2 times (also at ARC-0003)",
                "Labels: cache",
                "Reason: fixed",
                "",
                "# Cache never expires",
                "",
                "Details here.",
            ]
        );
        let untitled = NoteDto {
            title: String::new(),
            body: String::from("\n first line \nsecond"),
            ..note(5)
        };
        assert_eq!(title(&untitled), "first line");
        let empty = NoteDto {
            title: String::new(),
            ..note(6)
        };
        assert_eq!(title(&empty), "Note 6");
    }
}
