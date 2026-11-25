use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullradix_api::{
    ActorKindDto, AddQuestionComment, AnswerQuestion, CreateQuestion, DecideQuestion, NewOption,
    QuestionDetailDto, QuestionDto, QuestionStateDto,
};
use nullradix_client::Client;
use nullradix_domain::Timestamp;
use uuid::Uuid;

use crate::comment::{agent_session, body};
use crate::project;
use crate::selection::BoardChoice;

#[derive(Subcommand)]
pub(crate) enum QuestionCommand {
    /// Ask a person; the tickets it is about wait until it is decided
    Add(AddArgs),
    /// Comment on a question: a follow-up, an interim finding or a hint
    Comment(CommentArgs),
    /// List the questions of a project, ready ones first; only open ones without --state
    List(ListArgs),
    /// Show a question with its tickets, options, decision and comments
    Show {
        /// Question key such as ARC-Q7
        key: String,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Answer by an option of the newest round or an own text; people only
    Answer(AnswerArgs),
    /// Discard an open question with a reason; people only
    Discard(DecideArgs),
    /// Open an answered or discarded question again; people only
    Reopen(DecideArgs),
}

#[derive(clap::Args)]
pub(crate) struct DecideArgs {
    /// Question key such as ARC-Q7
    key: String,
    /// Why; discarding needs one
    #[arg(long)]
    reason: Option<String>,
    /// Name of the person who decides; the person the daemon acts for when left out
    #[arg(long)]
    author: Option<String>,
}

#[derive(clap::Args)]
pub(crate) struct AddArgs {
    title: String,
    /// Description in Markdown
    #[arg(long)]
    description: Option<String>,
    /// A ticket the question is about; repeat for more
    #[arg(long = "ticket")]
    tickets: Vec<String>,
    /// A question or ticket the answer waits for; repeat for more
    #[arg(long = "depends")]
    depends: Vec<String>,
    /// A tag; repeat for more
    #[arg(long = "tag")]
    tags: Vec<String>,
    /// Why it came up, such as "planning ARC-0007"
    #[arg(long)]
    occasion: Option<String>,
    /// An option as "title: text"; repeat for up to five
    #[arg(long = "option")]
    options: Vec<String>,
    /// The option to recommend, such as O2
    #[arg(long)]
    recommend: Option<String>,
    /// Project key; by default the project of the repository's board, else of the active one
    #[arg(long)]
    project: Option<String>,
    /// Who asks, such as claude; the person the daemon acts for when left out
    #[arg(long)]
    author: Option<String>,
}

#[derive(clap::Args)]
pub(crate) struct CommentArgs {
    /// Question key such as ARC-Q7
    key: String,
    #[arg(required_unless_present = "body_file", conflicts_with = "body_file")]
    text: Option<String>,
    /// Read the comment from this file; `-` reads standard input
    #[arg(long)]
    body_file: Option<PathBuf>,
    #[arg(long)]
    author: Option<String>,
}

#[derive(clap::Args)]
pub(crate) struct ListArgs {
    /// open, answered, discarded or all
    #[arg(long, default_value = "open")]
    state: String,
    /// Only questions about this ticket
    #[arg(long)]
    ticket: Option<String>,
    /// Project key; by default the project of the repository's board, else of the active one
    #[arg(long)]
    project: Option<String>,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
pub(crate) struct AnswerArgs {
    /// Question key such as ARC-Q7
    key: String,
    /// The chosen option of the newest round, such as O2
    #[arg(long, conflicts_with = "text")]
    option: Option<String>,
    /// An own answer instead of an option
    #[arg(long)]
    text: Option<String>,
    /// Why; optional
    #[arg(long)]
    reason: Option<String>,
    /// Name of the person who decides; the person the daemon acts for when left out
    #[arg(long)]
    author: Option<String>,
}

pub(crate) fn run(
    command: QuestionCommand,
    client: &Client,
    choice: &BoardChoice,
) -> anyhow::Result<ExitCode> {
    match command {
        QuestionCommand::Add(args) => add(args, client, choice)?,
        QuestionCommand::Comment(args) => {
            let input = AddQuestionComment {
                body: body(args.text, args.body_file.as_deref())?,
                author: args.author,
                session: agent_session(),
            };
            let comment = client.add_question_comment(args.key.trim(), &input)?;
            println!("Comment on {} saved.", comment.question_key);
        }
        QuestionCommand::List(args) => {
            let project = project_id(client, choice, args.project.as_deref())?;
            let mut query = vec![("state", args.state.as_str())];
            if let Some(ticket) = &args.ticket {
                query.push(("ticket", ticket.as_str()));
            }
            let list = client.list_questions(project, &query)?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                for line in list_lines(&list.items) {
                    println!("{line}");
                }
            }
        }
        QuestionCommand::Show { key, json } => {
            let detail = client.get_question(key.trim())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&detail)?);
            } else {
                for line in show_lines(&detail) {
                    println!("{line}");
                }
            }
        }
        QuestionCommand::Answer(args) => {
            let input = AnswerQuestion {
                option: args.option,
                text: args.text,
                reason: args.reason,
                version: None,
                author: args.author,
                session: agent_session(),
            };
            let detail = client.answer_question(args.key.trim(), &input)?;
            println!(
                "{} answered: {}",
                detail.question.key,
                decision_text(&detail)
            );
        }
        QuestionCommand::Discard(args) => {
            let detail = decide(client, "discard", args)?;
            println!("{} discarded.", detail.question.key);
        }
        QuestionCommand::Reopen(args) => {
            let detail = decide(client, "reopen", args)?;
            println!("{} reopened.", detail.question.key);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn decide(client: &Client, step: &str, args: DecideArgs) -> anyhow::Result<QuestionDetailDto> {
    let input = DecideQuestion {
        reason: args.reason,
        version: None,
        author: args.author,
        session: agent_session(),
    };
    Ok(client.decide_question(args.key.trim(), step, &input)?)
}

fn add(args: AddArgs, client: &Client, choice: &BoardChoice) -> anyhow::Result<()> {
    let project = project_id(client, choice, args.project.as_deref())?;
    let input = CreateQuestion {
        title: args.title,
        description: args.description,
        occasion: args.occasion,
        tags: args.tags,
        tickets: args.tickets,
        depends_on: args.depends,
        options: options(&args.options, args.recommend.as_deref()),
        author: args.author,
        session: agent_session(),
    };
    let created = client.create_question(project, &input)?;
    println!(
        "Question {} created: {}",
        created.question.key, created.question.title
    );
    Ok(())
}

/// Options typed as `title: text`; the recommended one is named by its key, such as `O2`.
fn options(typed: &[String], recommend: Option<&str>) -> Vec<NewOption> {
    let recommended = recommend.map(|key| key.trim().to_uppercase());
    typed
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let (title, text) = text.split_once(':').unwrap_or((text, ""));
            NewOption {
                title: title.trim().to_owned(),
                text: text.trim().to_owned(),
                pros: Vec::new(),
                cons: Vec::new(),
                recommended: recommended.as_deref() == Some(&format!("O{}", index + 1)),
            }
        })
        .collect()
}

/// `--project`, else the project of the board the CLI works on.
fn project_id(client: &Client, choice: &BoardChoice, key: Option<&str>) -> anyhow::Result<Uuid> {
    match key {
        Some(key) => Ok(project::find(client, key)?.id),
        None => Ok(choice.board(client, None)?.project_id),
    }
}

const fn state_name(state: QuestionStateDto) -> &'static str {
    match state {
        QuestionStateDto::Open => "open",
        QuestionStateDto::Answered => "answered",
        QuestionStateDto::Discarded => "discarded",
    }
}

fn counted(count: u32) -> String {
    if count == 1 {
        String::from("1 option")
    } else {
        format!("{count} options")
    }
}

fn or_dash(items: &[String]) -> String {
    if items.is_empty() {
        String::from("-")
    } else {
        items.join(", ")
    }
}

/// One line per question, as legacy: `  ARC-Q0001  Title  [ready] [2 options] -> ARC-0007`.
fn list_lines(questions: &[QuestionDto]) -> Vec<String> {
    if questions.is_empty() {
        return vec![String::from("(no questions found)")];
    }
    questions
        .iter()
        .map(|question| {
            let mut marks = Vec::new();
            if question.state != QuestionStateDto::Open {
                marks.push(format!("[{}]", state_name(question.state)));
            }
            if question.ready {
                marks.push(String::from("[ready]"));
            }
            if !question.waiting_on.is_empty() {
                let keys: Vec<&str> = question.waiting_on.iter().map(|w| w.key.as_str()).collect();
                marks.push(format!("[waits on {}]", keys.join(", ")));
            }
            if question.researched {
                marks.push(format!("[{}]", counted(question.option_count)));
            }
            if !question.tickets.is_empty() {
                let keys: Vec<&str> = question.tickets.iter().map(|t| t.key.as_str()).collect();
                marks.push(format!("-> {}", keys.join(", ")));
            }
            if marks.is_empty() {
                format!("  {}  {}", question.key, question.title)
            } else {
                format!(
                    "  {}  {}  {}",
                    question.key,
                    question.title,
                    marks.join(" ")
                )
            }
        })
        .collect()
}

const fn kind(kind: ActorKindDto) -> &'static str {
    match kind {
        ActorKindDto::Human => "person",
        ActorKindDto::Session => "session",
        ActorKindDto::Subagent => "subagent",
        ActorKindDto::Dispatcher => "dispatcher",
        ActorKindDto::System => "system",
    }
}

/// The decision in words, such as `option O2 "Db" chosen. Reason: fits`.
fn decision_text(detail: &QuestionDetailDto) -> String {
    let Some(decision) = &detail.question.decision else {
        return String::from("-");
    };
    let because = decision
        .reason
        .as_deref()
        .map_or_else(String::new, |reason| format!(" Reason: {reason}"));
    if detail.question.state == QuestionStateDto::Discarded {
        return format!("discarded.{because}");
    }
    match (&decision.option, &decision.text) {
        (Some(option), _) => {
            let title = detail
                .research
                .iter()
                .rev()
                .flat_map(|round| &round.options)
                .find(|known| &known.key == option)
                .map_or_else(String::new, |known| format!(" \"{}\"", known.title));
            format!("option {option}{title} chosen.{because}")
        }
        (None, text) => format!("answer: {}.{because}", text.as_deref().unwrap_or_default()),
    }
}

/// The rounds of options, each option with its pros and cons.
fn option_lines(detail: &QuestionDetailDto) -> Vec<String> {
    let mut lines = Vec::new();
    if detail.research.is_empty() {
        lines.push(String::from("Options: none yet"));
    } else {
        lines.push(String::from("Options:"));
        for round in &detail.research {
            if !round.context.trim().is_empty() {
                lines.extend(
                    round
                        .context
                        .trim_end()
                        .lines()
                        .map(|line| format!("    {line}")),
                );
            }
            for option in &round.options {
                let recommended = if option.recommended {
                    " (recommended)"
                } else {
                    ""
                };
                lines.push(format!(
                    "  {}{recommended}: {} - {}",
                    option.key, option.title, option.text
                ));
                lines.extend(option.pros.iter().map(|pro| format!("      + {pro}")));
                lines.extend(option.cons.iter().map(|con| format!("      - {con}")));
            }
        }
    }
    lines
}

fn show_lines(detail: &QuestionDetailDto) -> Vec<String> {
    let question = &detail.question;
    let ready = if question.ready {
        " (ready to decide)"
    } else {
        ""
    };
    let session = question
        .origin_session
        .as_ref()
        .map_or_else(String::new, |session| format!(", session {session}"));
    let tickets: Vec<String> = question
        .tickets
        .iter()
        .map(|ticket| format!("{} {}", ticket.key, ticket.title))
        .collect();
    let waiting: Vec<String> = question
        .waiting_on
        .iter()
        .map(|w| format!("{} ({})", w.key, w.state))
        .collect();
    let description = question.description.trim_end();
    let mut lines = vec![
        format!("{}  {}", question.key, question.title),
        format!(
            "State: {}{ready} | version {}",
            state_name(question.state),
            question.version
        ),
        format!("Tags: {}", or_dash(&question.tags)),
        format!(
            "Asked by: {} ({}){session}",
            question.origin_name,
            kind(question.origin_kind)
        ),
        format!("Occasion: {}", question.occasion.as_deref().unwrap_or("-")),
        format!("About: {}", or_dash(&tickets)),
        format!("Depends on: {}", or_dash(&question.depends_on)),
        format!("Waits on: {}", or_dash(&waiting)),
        String::new(),
        if description.is_empty() {
            String::from("(no description)")
        } else {
            description.to_owned()
        },
        String::new(),
    ];
    lines.extend(option_lines(detail));
    lines.push(String::new());
    if let Some(decision) = &question.decision {
        let autopilot = if decision.autopilot {
            " (by the autopilot)"
        } else {
            ""
        };
        lines.push(format!("Decision: {}{autopilot}", decision_text(detail)));
        lines.push(format!(
            "  by {}, {}",
            decision.by_name,
            Timestamp::from_millis(decision.at)
        ));
        lines.push(String::new());
    }
    lines.push(String::from("Comments:"));
    if detail.comments.is_empty() {
        lines.push(String::from("  (none)"));
    }
    for comment in &detail.comments {
        lines.push(format!(
            "  {}  {} ({}): {}",
            Timestamp::from_millis(comment.created_at),
            comment.author_name,
            kind(comment.author_kind),
            comment.body
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: QuestionCommand,
    }

    #[test]
    fn options_split_title_and_text_and_mark_the_recommended_one() {
        let typed = [
            String::from("File: one file"),
            String::from("Db :SQLite: local"),
        ];
        let built = options(&typed, Some("o2"));
        assert_eq!(built[0].title, "File");
        assert_eq!(built[0].text, "one file");
        assert_eq!(
            (built[1].title.as_str(), built[1].text.as_str()),
            ("Db", "SQLite: local")
        );
        assert_eq!(
            built
                .iter()
                .map(|option| option.recommended)
                .collect::<Vec<_>>(),
            [false, true]
        );
        let bare = options(&[String::from("Only a title")], None);
        assert_eq!(bare[0].text, "");
    }

    #[test]
    fn answers_take_an_option_or_a_text_but_not_both() {
        let parse =
            |args: &[&str]| Cli::try_parse_from([&["question", "answer", "ARC-Q1"], args].concat());
        assert!(parse(&["--option", "O2", "--reason", "fits"]).is_ok());
        assert!(parse(&["--text", "neither"]).is_ok());
        let both = parse(&["--option", "O1", "--text", "x"]).err().unwrap();
        assert_eq!(both.kind(), clap::error::ErrorKind::ArgumentConflict);
        let discard = Cli::try_parse_from(["question", "discard", "ARC-Q1", "--reason", "moot"]);
        assert!(matches!(
            discard.unwrap().command,
            QuestionCommand::Discard(DecideArgs { ref reason, .. }) if reason.as_deref() == Some("moot")
        ));
        assert!(Cli::try_parse_from(["question", "reopen", "ARC-Q1"]).is_ok());
        let list = Cli::try_parse_from(["question", "list"]).unwrap();
        assert!(
            matches!(list.command, QuestionCommand::List(ListArgs { ref state, .. }) if state == "open")
        );
    }

    #[test]
    fn an_empty_list_says_so() {
        assert_eq!(list_lines(&[]), ["(no questions found)"]);
        assert_eq!(counted(1), "1 option");
        assert_eq!(or_dash(&[]), "-");
    }
}
