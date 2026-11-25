use std::process::ExitCode;

use clap::Subcommand;
use nullradix_api::{
    ChangeOriginDto, DiffLineKindDto, DisplayStateDto, FavoriteDto, FileDiffDto, FileStatusDto,
    SessionChangesDto, SessionClientDto, SessionDetailDto, SessionDto, SubagentStateDto,
    SystemLevelDto, TodoStatusDto, TranscriptEntryDto,
};
use nullradix_client::Client;
use nullradix_domain::Timestamp;
use uuid::Uuid;

/// Characters of a session id lists show, as legacy did.
const SHORT_ID: usize = 8;

#[derive(Subcommand)]
pub(crate) enum SessionCommand {
    /// List Claude Code sessions, latest activity first
    #[command(
        after_help = "Examples:\n  nullradix session list --state running\n  nullradix session list --ticket ARC-7"
    )]
    List {
        /// Only sessions in this repository id
        #[arg(long)]
        repository: Option<Uuid>,
        /// idle, running, waiting, ended or stale
        #[arg(long)]
        state: Option<String>,
        /// Only sessions linked to this ticket key
        #[arg(long)]
        ticket: Option<String>,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Show one session with its subagents, tickets and the figures of its transcript
    Show {
        /// Claude Code's session id
        session_id: String,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Print what the session's transcript holds, one line per entry
    #[command(after_help = "Example:\n  nullradix session transcript 0123abcd --after 4096")]
    Transcript {
        /// Claude Code's session id
        session_id: String,
        /// Byte offset to start after, from a previous cursor
        #[arg(long, default_value_t = 0)]
        after: u64,
        /// Entries of one page; without it every page is read
        #[arg(long)]
        limit: Option<u32>,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// List the files the session changed, from git or else its transcript
    Changes {
        /// Claude Code's session id
        session_id: String,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Mark a session as favorite, kept to resume later; marking again updates it
    #[command(after_help = "Example:\n  nullradix session favorite 0123abcd --note \"login fix\"")]
    Favorite {
        /// Claude Code's session id
        session_id: String,
        /// Note on why the session matters; replaces the old one
        #[arg(long)]
        note: Option<String>,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Remove the favorite mark of a session
    Unfavorite {
        /// Claude Code's session id
        session_id: String,
    },
    /// List favorite sessions with the command that resumes each
    Favorites {
        /// Only favorites in this repository id
        #[arg(long)]
        repository: Option<Uuid>,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Show the diff of one file the session changed
    Diff {
        /// Claude Code's session id
        session_id: String,
        /// Path relative to the checkout, as `session changes` lists it
        path: String,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run(command: SessionCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        SessionCommand::List {
            repository,
            state,
            ticket,
            json,
        } => {
            let repository = repository.map(|id| id.to_string());
            let mut query = Vec::new();
            if let Some(repository) = repository.as_deref() {
                query.push(("repositoryId", repository));
            }
            if let Some(state) = state.as_deref() {
                query.push(("state", state));
            }
            if let Some(ticket) = ticket.as_deref() {
                query.push(("ticket", ticket));
            }
            let sessions = client.list_sessions(&query)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
            } else {
                for line in list_lines(&sessions.items) {
                    println!("{line}");
                }
            }
        }
        SessionCommand::Show { session_id, json } => {
            let detail = client.session_detail(&session_id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&detail)?);
            } else {
                print_lines(&detail_lines(&detail));
            }
        }
        SessionCommand::Transcript {
            session_id,
            after,
            limit,
            json,
        } => transcript(client, &session_id, after, limit, json)?,
        SessionCommand::Changes { session_id, json } => {
            let changes = client.session_changes(&session_id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&changes)?);
            } else {
                print_lines(&change_lines(&changes));
            }
        }
        SessionCommand::Favorite {
            session_id,
            note,
            json,
        } => {
            let favorite = client.mark_favorite(&session_id, note.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&favorite)?);
            } else {
                println!("{} is a favorite.", short(&favorite.session_id));
                print_lines(&favorite_lines(&favorite));
            }
        }
        SessionCommand::Unfavorite { session_id } => {
            client.unmark_favorite(&session_id)?;
            println!("{} is no favorite.", short(&session_id));
        }
        SessionCommand::Favorites { repository, json } => {
            let repository = repository.map(|id| id.to_string());
            let favorites = client.list_favorites(repository.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&favorites)?);
            } else if favorites.items.is_empty() {
                println!("No favorites.");
            } else {
                for favorite in &favorites.items {
                    print_lines(&favorite_lines(favorite));
                }
            }
        }
        SessionCommand::Diff {
            session_id,
            path,
            json,
        } => {
            let diff = client.session_diff(&session_id, &path)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&diff)?);
            } else {
                print_lines(&diff_lines(&diff));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// State, name, short id and tickets, then note, folder, resumptions and the command.
fn favorite_lines(favorite: &FavoriteDto) -> Vec<String> {
    let name = if favorite.label.is_empty() {
        "(untitled)"
    } else {
        &favorite.label
    };
    let tickets = if favorite.tickets.is_empty() {
        String::from("-")
    } else {
        favorite.tickets.join(",")
    };
    let mut lines = vec![format!(
        "{:<8} {name}  [{}]  {tickets}{}",
        state_label(favorite.display_state),
        short(&favorite.session_id),
        if favorite.live { "" } else { "  (gone)" },
    )];
    if !favorite.note.is_empty() {
        lines.push(format!("    note: {}", favorite.note));
    }
    let missing = if favorite.cwd_exists {
        ""
    } else {
        " (missing)"
    };
    lines.push(format!("    folder: {}{missing}", favorite.cwd));
    if !favorite.resumed_as.is_empty() {
        let resumed: Vec<&str> = favorite
            .resumed_as
            .iter()
            .map(|resumption| short(&resumption.session_id))
            .collect();
        lines.push(format!("    resumed as: {}", resumed.join(", ")));
    }
    lines.push(format!("    {}", favorite.command));
    lines
}

fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

/// One page with a limit, else every page until the cursor stops.
fn transcript(
    client: &Client,
    id: &str,
    after: u64,
    limit: Option<u32>,
    json: bool,
) -> anyhow::Result<()> {
    let mut cursor = after;
    let mut entries = Vec::new();
    loop {
        let page = client.session_transcript(id, cursor, limit)?;
        if !page.available {
            if json {
                println!("{}", serde_json::to_string_pretty(&page)?);
            } else {
                println!("The transcript is not available.");
            }
            return Ok(());
        }
        let done = limit.is_some() || page.entries.is_empty() || page.cursor <= cursor;
        cursor = page.cursor;
        if json && limit.is_some() {
            println!("{}", serde_json::to_string_pretty(&page)?);
            return Ok(());
        }
        merge(&mut entries, page.entries);
        if done {
            break;
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        print_lines(&transcript_lines(&entries));
        if limit.is_some() {
            println!("Next page: --after {cursor}");
        }
    }
    Ok(())
}

fn entry_id(entry: &TranscriptEntryDto) -> &str {
    match entry {
        TranscriptEntryDto::Prompt { id, .. }
        | TranscriptEntryDto::Text { id, .. }
        | TranscriptEntryDto::Thinking { id, .. }
        | TranscriptEntryDto::Tool { id, .. }
        | TranscriptEntryDto::System { id, .. } => id,
    }
}

/// Adds a page; an entry with a known id replaces the earlier one, as pages promise.
fn merge(entries: &mut Vec<TranscriptEntryDto>, page: Vec<TranscriptEntryDto>) {
    for entry in page {
        match entries
            .iter()
            .position(|known| entry_id(known) == entry_id(&entry))
        {
            Some(index) => entries[index] = entry,
            None => entries.push(entry),
        }
    }
}

fn first_line(text: &str) -> &str {
    text.trim().lines().next().unwrap_or_default()
}

fn transcript_lines(entries: &[TranscriptEntryDto]) -> Vec<String> {
    if entries.is_empty() {
        return vec![String::from("No entries.")];
    }
    entries
        .iter()
        .map(|entry| match entry {
            TranscriptEntryDto::Prompt { text, .. } => format!("prompt   {}", first_line(text)),
            TranscriptEntryDto::Text { text, .. } => format!("answer   {}", first_line(text)),
            TranscriptEntryDto::Thinking { duration_ms, .. } => match duration_ms {
                Some(millis) => format!("thinking {millis} ms"),
                None => String::from("thinking"),
            },
            TranscriptEntryDto::Tool {
                summary, result, ..
            } => {
                let state = match result {
                    None => " (running)",
                    Some(result) if result.is_error => " (failed)",
                    Some(_) => "",
                };
                format!("tool     {summary}{state}")
            }
            TranscriptEntryDto::System { text, level, .. } => {
                let level = match level {
                    SystemLevelDto::Info => "info",
                    SystemLevelDto::Warning => "warning",
                    SystemLevelDto::Error => "error",
                };
                format!("{level:<8} {}", first_line(text))
            }
        })
        .collect()
}

fn client_label(client: SessionClientDto) -> &'static str {
    match client {
        SessionClientDto::Nullradix => "nullradix",
        SessionClientDto::ClaudeCode => "claude-code",
        SessionClientDto::ClaudeDesktop => "claude-desktop",
        SessionClientDto::Unknown => "unknown",
    }
}

/// The session as `show_lines` prints it plus the figures of its transcript.
fn detail_lines(detail: &SessionDetailDto) -> Vec<String> {
    let mut lines = show_lines(&detail.session);
    let mut facts = vec![format!("Client:     {}", client_label(detail.client))];
    if let Some(branch) = &detail.branch {
        facts.push(format!("Branch:     {branch}"));
    }
    if let Some(version) = &detail.claude_version {
        facts.push(format!("Claude:     {version}"));
    }
    let tokens = &detail.tokens;
    facts.push(format!(
        "Tokens:     {} in, {} out, {} cache read, {} cache write",
        tokens.input, tokens.output, tokens.cache_read, tokens.cache_write
    ));
    if let Some(cost) = detail.cost_usd {
        facts.push(format!("Cost:       ${cost:.2}"));
    }
    if let Some(context) = &detail.context {
        let limit = context
            .limit_tokens
            .map_or_else(String::new, |limit| format!(" of {limit}"));
        facts.push(format!("Context:    {}{limit} tokens", context.used_tokens));
    }
    facts.push(
        match (&detail.transcript.path, detail.transcript.size_bytes) {
            (Some(path), Some(size)) if detail.transcript.available => {
                format!("Transcript: {path} ({size} bytes)")
            }
            _ => String::from("Transcript: not available"),
        },
    );
    if !detail.todos.is_empty() {
        facts.push(String::from("Todos:"));
        for todo in &detail.todos {
            let mark = match todo.status {
                TodoStatusDto::Pending => ' ',
                TodoStatusDto::InProgress => '>',
                TodoStatusDto::Completed => 'x',
            };
            facts.push(format!("  [{mark}] {}", todo.content));
        }
    }
    // the figures go before the subagents, which end the list
    let at = lines
        .iter()
        .position(|line| line == "Subagents:")
        .unwrap_or(lines.len());
    lines.splice(at..at, facts);
    lines
}

fn change_lines(changes: &SessionChangesDto) -> Vec<String> {
    let mut lines = vec![match changes.origin {
        ChangeOriginDto::Git => format!(
            "Changes from git against {}",
            changes.base.as_deref().unwrap_or("HEAD")
        ),
        ChangeOriginDto::Transcript => String::from("Changes from the transcript"),
        ChangeOriginDto::None => String::from("No source of changes."),
    }];
    if let Some(note) = &changes.note {
        lines.push(note.clone());
    }
    for file in &changes.files {
        let status = match file.status {
            FileStatusDto::Added => 'A',
            FileStatusDto::Modified => 'M',
            FileStatusDto::Deleted => 'D',
            FileStatusDto::Renamed => 'R',
        };
        lines.push(format!(
            "{status} {} (+{} -{})",
            file.path, file.added, file.removed
        ));
    }
    if changes.files.is_empty() && changes.origin != ChangeOriginDto::None {
        lines.push(String::from("No changed files."));
    }
    lines
}

fn diff_lines(diff: &FileDiffDto) -> Vec<String> {
    let mut lines = vec![format!("--- {}", diff.path)];
    for hunk in &diff.hunks {
        lines.push(hunk.header.clone());
        for line in &hunk.lines {
            let sign = match line.kind {
                DiffLineKindDto::Add => '+',
                DiffLineKindDto::Remove => '-',
                DiffLineKindDto::Context => ' ',
            };
            lines.push(format!("{sign}{}", line.text));
        }
    }
    if diff.truncated {
        lines.push(String::from("(cut after 5000 lines)"));
    }
    lines
}

fn state_label(state: DisplayStateDto) -> &'static str {
    match state {
        DisplayStateDto::Idle => "idle",
        DisplayStateDto::Running => "running",
        DisplayStateDto::Waiting => "waiting",
        DisplayStateDto::Ended => "ended",
        DisplayStateDto::Stale => "stale",
    }
}

/// `less than 1 min`, `5 min`, `1 h 05 min` or `2 d 3 h`, as the legacy list.
fn duration(millis: i64) -> String {
    let minutes = millis / 60_000;
    if minutes < 1 {
        return String::from("less than 1 min");
    }
    if minutes < 60 {
        return format!("{minutes} min");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours} h {:02} min", minutes % 60);
    }
    format!("{} d {} h", hours / 24, hours % 24)
}

fn short(id: &str) -> &str {
    id.char_indices()
        .nth(SHORT_ID)
        .map_or(id, |(end, _)| &id[..end])
}

fn title(session: &SessionDto) -> &str {
    if session.label.is_empty() {
        "(untitled)"
    } else {
        &session.label
    }
}

fn tickets(session: &SessionDto) -> String {
    session
        .tickets
        .iter()
        .map(|ticket| ticket.key.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// State, runtime, title, short id, tickets, last activity and folder; running subagents below.
fn list_lines(sessions: &[SessionDto]) -> Vec<String> {
    if sessions.is_empty() {
        return vec![String::from("No sessions.")];
    }
    let mut lines = Vec::new();
    for session in sessions {
        let linked = tickets(session);
        let linked = if linked.is_empty() {
            String::from("-")
        } else {
            linked
        };
        lines.push(format!(
            "{:<8} {:<16} {}  [{}]  {}  last {}  {}",
            state_label(session.display_state),
            duration(session.runtime_ms),
            title(session),
            short(&session.id),
            linked,
            Timestamp::from_millis(session.last_event_at),
            session.cwd,
        ));
        for agent in &session.subagents {
            if agent.state == SubagentStateDto::Running {
                lines.push(format!("    agent {} {}", agent.kind, agent.id));
            }
        }
    }
    lines
}

fn show_lines(session: &SessionDto) -> Vec<String> {
    let optional = |value: Option<&str>| value.unwrap_or("-").to_owned();
    let mut lines = vec![
        format!("{} ({})", title(session), session.id),
        format!("State:      {}", state_label(session.display_state)),
        format!("Runtime:    {}", duration(session.runtime_ms)),
        format!("Started:    {}", Timestamp::from_millis(session.started_at)),
        format!(
            "Last hook:  {}",
            Timestamp::from_millis(session.last_event_at)
        ),
        format!("Folder:     {}", session.cwd),
        format!("Model:      {}", optional(session.model.as_deref())),
    ];
    if let Some(ended) = session.ended_at {
        lines.insert(5, format!("Ended:      {}", Timestamp::from_millis(ended)));
    }
    let linked = tickets(session);
    if !linked.is_empty() {
        lines.push(format!("Tickets:    {linked}"));
    }
    if let Some(run) = &session.run_name {
        lines.push(format!("Run:        {run}"));
    }
    if !session.subagents.is_empty() {
        lines.push(String::from("Subagents:"));
        for agent in &session.subagents {
            let state = match agent.state {
                SubagentStateDto::Running => "running",
                SubagentStateDto::Ended => "ended",
            };
            lines.push(format!("  {} {} ({state})", agent.kind, agent.id));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use nullradix_api::{
        ChangedFileDto, DiffHunkDto, DiffLineDto, ResumptionDto, SessionStateDto, SubagentDto,
        TodoItemDto, TokenTotalsDto, ToolResultDto, TranscriptInfoDto,
    };

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: SessionCommand,
    }

    #[test]
    fn parses_list_filters_and_show() {
        let cli = Cli::try_parse_from(["session", "list", "--state", "running", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            SessionCommand::List { state: Some(state), json: true, .. } if state == "running"
        ));
        let cli = Cli::try_parse_from(["session", "show", "abc"]).unwrap();
        assert!(matches!(
            cli.command,
            SessionCommand::Show { session_id, json: false } if session_id == "abc"
        ));
        assert!(Cli::try_parse_from(["session", "list", "--repository", "x"]).is_err());
        assert!(Cli::try_parse_from(["session", "show"]).is_err());
        let cli = Cli::try_parse_from(["session", "transcript", "abc", "--after", "12"]).unwrap();
        assert!(matches!(
            cli.command,
            SessionCommand::Transcript {
                after: 12,
                limit: None,
                ..
            }
        ));
        let cli = Cli::try_parse_from(["session", "diff", "abc", "src/a.rs"]).unwrap();
        assert!(matches!(cli.command, SessionCommand::Diff { path, .. } if path == "src/a.rs"));
        assert!(Cli::try_parse_from(["session", "diff", "abc"]).is_err());
    }

    #[test]
    fn durations_read_like_the_legacy_list() {
        assert_eq!(duration(59_000), "less than 1 min");
        assert_eq!(duration(5 * 60_000), "5 min");
        assert_eq!(duration(65 * 60_000), "1 h 05 min");
        assert_eq!(duration(51 * 60 * 60_000), "2 d 3 h");
    }

    fn session() -> SessionDto {
        SessionDto {
            id: String::from("0123456789abcdef"),
            repository_id: None,
            runner_id: None,
            number: Some(1),
            label: String::from("SE-001: Fix the login"),
            title: Some(String::from("Fix the login")),
            cwd: String::from("/repo"),
            state: SessionStateDto::Running,
            display_state: DisplayStateDto::Running,
            listening: true,
            started_at: 0,
            last_event_at: 60_000,
            ended_at: None,
            runtime_ms: 60_000,
            model: None,
            transcript_path: None,
            client: SessionClientDto::ClaudeCode,
            run_name: None,
            subagents: vec![SubagentDto {
                id: String::from("a1"),
                kind: String::from("Explore"),
                state: SubagentStateDto::Running,
                started_at: 0,
                ended_at: None,
            }],
            tickets: Vec::new(),
            version: 3,
        }
    }

    #[test]
    fn lists_one_line_per_session_with_running_agents_below() {
        assert_eq!(
            list_lines(&[session()]),
            [
                "running  1 min            SE-001: Fix the login  [01234567]  -  last 1970-01-01T00:01:00.000Z  /repo",
                "    agent Explore a1",
            ]
        );
        assert_eq!(list_lines(&[]), ["No sessions."]);
    }

    fn detail() -> SessionDetailDto {
        SessionDetailDto {
            session: session(),
            client: SessionClientDto::ClaudeDesktop,
            transcript: TranscriptInfoDto {
                available: true,
                path: Some(String::from("/t/s.jsonl")),
                size_bytes: Some(10),
            },
            branch: Some(String::from("feature/x")),
            claude_version: None,
            tokens: TokenTotalsDto {
                input: 1,
                output: 2,
                cache_read: 3,
                cache_write: 4,
            },
            cost_usd: Some(0.421),
            duration_ms: None,
            context: None,
            todos: vec![TodoItemDto {
                content: String::from("Write tests"),
                status: TodoStatusDto::InProgress,
            }],
        }
    }

    #[test]
    fn the_detail_adds_the_transcript_figures_before_the_subagents() {
        let lines = detail_lines(&detail());
        let at = |text: &str| lines.iter().position(|line| line == text).unwrap();
        assert!(at("Client:     claude-desktop") < at("Subagents:"));
        assert!(lines.contains(&String::from("Cost:       $0.42")));
        assert!(lines.contains(&String::from("Transcript: /t/s.jsonl (10 bytes)")));
        assert!(lines.contains(&String::from("  [>] Write tests")));
        assert!(lines.contains(&String::from("Branch:     feature/x")));
    }

    #[test]
    fn transcript_pages_replace_entries_with_known_ids() {
        let tool = |result: Option<ToolResultDto>| TranscriptEntryDto::Tool {
            id: String::from("t1"),
            at: String::new(),
            name: String::from("Bash"),
            summary: String::from("Bash: ls"),
            input: serde_json::json!({ "command": "ls" }),
            result,
        };
        let mut entries = Vec::new();
        merge(&mut entries, vec![tool(None)]);
        assert_eq!(transcript_lines(&entries), ["tool     Bash: ls (running)"]);
        let failed = ToolResultDto {
            text: String::from("no"),
            is_error: true,
            truncated: false,
        };
        merge(
            &mut entries,
            vec![
                tool(Some(failed)),
                TranscriptEntryDto::Prompt {
                    id: String::from("p"),
                    at: String::new(),
                    text: String::from("Fix it\nplease"),
                },
            ],
        );
        assert_eq!(
            transcript_lines(&entries),
            ["tool     Bash: ls (failed)", "prompt   Fix it"]
        );
        assert_eq!(transcript_lines(&[]), ["No entries."]);
    }

    #[test]
    fn changes_and_diffs_read_like_git() {
        let changes = SessionChangesDto {
            origin: ChangeOriginDto::Git,
            base: Some(String::from("main")),
            note: None,
            files: vec![ChangedFileDto {
                path: String::from("src/a.rs"),
                status: FileStatusDto::Modified,
                added: 2,
                removed: 1,
            }],
        };
        assert_eq!(
            change_lines(&changes),
            ["Changes from git against main", "M src/a.rs (+2 -1)"]
        );
        let diff = FileDiffDto {
            path: String::from("src/a.rs"),
            hunks: vec![DiffHunkDto {
                header: String::from("@@ -1,1 +1,1 @@"),
                lines: vec![
                    DiffLineDto {
                        kind: DiffLineKindDto::Remove,
                        text: String::from("a"),
                    },
                    DiffLineDto {
                        kind: DiffLineKindDto::Add,
                        text: String::from("b"),
                    },
                ],
            }],
            truncated: false,
        };
        assert_eq!(
            diff_lines(&diff),
            ["--- src/a.rs", "@@ -1,1 +1,1 @@", "-a", "+b"]
        );
    }

    #[test]
    fn favorites_show_note_folder_resumptions_and_command() {
        let favorite = FavoriteDto {
            session_id: String::from("0123456789abcdef"),
            repository_id: None,
            note: String::from("login fix"),
            tickets: vec![String::from("ARC-0001")],
            cwd: String::from("/repo"),
            cwd_exists: false,
            title: Some(String::from("Fix the login")),
            label: String::from("SE-001: Fix the login"),
            marked_at: 0,
            state: SessionStateDto::Ended,
            display_state: DisplayStateDto::Ended,
            live: false,
            last_event_at: 0,
            resumed_as: vec![ResumptionDto {
                session_id: String::from("fedcba9876543210"),
                at: 0,
            }],
            command: String::from("cd \"/repo\" && claude --resume 0123456789abcdef"),
            version: 1,
        };
        assert_eq!(
            favorite_lines(&favorite),
            [
                "ended    SE-001: Fix the login  [01234567]  ARC-0001  (gone)",
                "    note: login fix",
                "    folder: /repo (missing)",
                "    resumed as: fedcba98",
                "    cd \"/repo\" && claude --resume 0123456789abcdef",
            ]
        );
        let cli = Cli::try_parse_from(["session", "favorite", "abc", "--note", "x"]).unwrap();
        assert!(matches!(
            cli.command,
            SessionCommand::Favorite { note: Some(note), .. } if note == "x"
        ));
        assert!(Cli::try_parse_from(["session", "unfavorite"]).is_err());
    }

    #[test]
    fn shows_the_session_in_full() {
        let lines = show_lines(&session());
        assert_eq!(lines[0], "SE-001: Fix the login (0123456789abcdef)");
        assert_eq!(
            lines.last().map(String::as_str),
            Some("  Explore a1 (running)")
        );
    }
}
