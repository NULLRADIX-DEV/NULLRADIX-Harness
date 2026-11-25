//! Claude runs of the dispatcher: starting one from the configuration and reading what it left.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    DeniedTool, DispatchRun, Id, Profile, QuestionKey, Run, RunKind, RunProfile, RunTrigger,
    TicketKey,
};
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::invocation::{InvocationInput, build, effective_effort, effective_model};
use super::stream::{DROPPED_LINE, Joined, LineJoiner, Pipe};
use super::transcript::{LineKind, TranscriptFormatter, TranscriptLine};
use crate::AppError;
use crate::accounts::AccountService;
use crate::dispatcher_config::DispatcherConfigService;
use crate::questions::{self, QuestionRepository};
use crate::runs::{OutputLine, OutputStream, RunArtifact, RunFilter, RunLink, RunService, RunSpec};
use crate::tickets::{self, TicketRepository};

/// Output lines read per transcript page at most.
pub const MAX_TRANSCRIPT_LINES: u32 = 1000;

/// Transcript pages formatted at once; each can hold a joined line of megabytes.
const FORMAT_PERMITS: usize = 4;
static FORMATTING: Semaphore = Semaphore::const_new(FORMAT_PERMITS);

/// What the scheduler, or a person, asks for when a Claude run should start.
#[derive(Debug, Clone)]
pub struct StartDispatchRun {
    pub kind: RunKind,
    pub trigger: RunTrigger,
    /// Ticket key, or question key for research.
    pub subject: String,
    pub prompt: String,
    pub cwd: PathBuf,
    /// A profile by name; none takes the one of the kind, else the default.
    pub profile: Option<String>,
    pub account: Option<Id<nullradix_domain::Account>>,
    pub extra_tools: Vec<String>,
}

/// Bytes of files a run left on its runner.
#[async_trait]
pub trait RunFilesPort: Send + Sync {
    /// The file of a screenshot; `None` once it is gone.
    async fn read_screenshot(&self, run: Id<Run>, file: &str) -> Result<Option<Vec<u8>>, AppError>;
}

/// A run with the rows that belong to it.
#[derive(Debug, Clone, PartialEq)]
pub struct RunDetail {
    pub run: Run,
    pub denied: Vec<DeniedTool>,
    pub artifacts: Vec<RunArtifact>,
}

/// The ticket or question a run list is narrowed to, as the caller spelled its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunSubject<'a> {
    Ticket(&'a str),
    Question(&'a str),
}

/// A readable line and the output line it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    pub seq: i64,
    pub line: TranscriptLine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptPage {
    pub entries: Vec<TranscriptEntry>,
    /// The last output line read; ask for lines after it.
    pub last_seq: i64,
    /// The run ended and no output is left.
    pub finished: bool,
    /// The read stopped at its limit; ask again at once.
    pub more: bool,
}

#[must_use]
pub fn profile_unknown(name: &str) -> AppError {
    AppError::invalid(
        "dispatch-profile-unknown",
        format!("there is no profile named \"{name}\""),
    )
}

fn artifact_not_found(run: Id<Run>) -> AppError {
    AppError::not_found(
        "run-artifact-not-found",
        format!("run {run} has no such screenshot"),
    )
}

pub struct DispatchRuns {
    runs: Arc<RunService>,
    config: Arc<DispatcherConfigService>,
    accounts: Arc<AccountService>,
    tickets: Arc<dyn TicketRepository>,
    questions: Arc<dyn QuestionRepository>,
    files: Arc<dyn RunFilesPort>,
}

impl std::fmt::Debug for DispatchRuns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchRuns").finish_non_exhaustive()
    }
}

fn profile_of<'a>(
    config: &'a nullradix_domain::DispatcherConfig,
    request: &StartDispatchRun,
) -> Result<&'a Profile, AppError> {
    let name = request
        .profile
        .as_deref()
        .or(match request.kind {
            RunKind::Research => config.research.profile.as_deref(),
            RunKind::Plan => config.planning.profile.as_deref(),
            RunKind::Work | RunKind::Merge => None,
        })
        .unwrap_or(&config.default_profile);
    config.profile(name).ok_or_else(|| profile_unknown(name))
}

fn run_profile(config: &nullradix_domain::DispatcherConfig, profile: &Profile) -> RunProfile {
    RunProfile {
        id: profile.id,
        name: profile.name.clone(),
        model: effective_model(&config.claude, profile),
        effort: effective_effort(&config.claude, profile),
        max_turns: u32::try_from(profile.max_turns).unwrap_or(u32::MAX),
        timeout_minutes: u32::try_from(profile.timeout_minutes).unwrap_or(u32::MAX),
    }
}

impl DispatchRuns {
    #[must_use]
    pub fn new(
        runs: Arc<RunService>,
        config: Arc<DispatcherConfigService>,
        accounts: Arc<AccountService>,
        tickets: Arc<dyn TicketRepository>,
        questions: Arc<dyn QuestionRepository>,
        files: Arc<dyn RunFilesPort>,
    ) -> Self {
        Self {
            runs,
            config,
            accounts,
            tickets,
            questions,
            files,
        }
    }

    /// The key of the ticket or question a run is about, which must exist.
    async fn subject(&self, kind: RunKind, text: &str) -> Result<String, AppError> {
        if kind.is_about_question() {
            let key = QuestionKey::parse(text).map_err(|error| questions::invalid(&error))?;
            if !self.questions.exists(&key).await? {
                return Err(questions::not_found(&key));
            }
            Ok(key.to_string())
        } else {
            let key = TicketKey::parse(text).map_err(|error| tickets::invalid(&error))?;
            if self.tickets.get(&key).await?.is_none() {
                return Err(tickets::not_found(&key));
            }
            Ok(key.to_string())
        }
    }

    /// Starts a Claude run from the configuration in force; progress arrives as events.
    pub async fn start(&self, request: StartDispatchRun) -> Result<Run, AppError> {
        if request.prompt.trim().is_empty() {
            return Err(AppError::invalid(
                "run-prompt-invalid",
                "the prompt is empty",
            ));
        }
        let subject = self.subject(request.kind, &request.subject).await?;
        self.config.sync_hash().await?;
        let view = self.config.get().await?;
        let profile = profile_of(&view.config, &request)?;
        let account = match request.account {
            Some(id) => Some(self.accounts.get(id).await?),
            None => None,
        };
        let id = self.runs.next_id();
        let run_id = id.to_string();
        let invocation = build(&InvocationInput {
            kind: request.kind,
            claude: &view.config.claude,
            profile,
            extra_tools: &request.extra_tools,
            ticket_key: (!request.kind.is_about_question()).then_some(subject.as_str()),
            run_id: &run_id,
            config_dir: account
                .as_ref()
                .and_then(|account| account.config_dir())
                .map(nullradix_domain::ConfigDir::as_str),
            git_bash: view.config.claude.git_bash_path.as_deref(),
        });
        let spec = RunSpec {
            program: invocation.program,
            args: invocation.args.clone(),
            cwd: request.cwd.clone(),
            timeout: Some(invocation.timeout),
            env: invocation.env,
            stdin: Some(request.prompt.clone()),
            claude: true,
        };
        let dispatch = DispatchRun {
            kind: request.kind,
            trigger: request.trigger,
            subject_key: Some(subject.clone()),
            profile: run_profile(&view.config, profile),
            account: account.as_ref().map(|account| account.id().as_uuid()),
            config_hash: view.hash,
            cwd: request.cwd.to_string_lossy().into_owned(),
            args: invocation.args,
            prompt: request.prompt,
        };
        let label = format!("{} {subject}", request.kind.as_str());
        self.runs
            .start_planned(id, &label, &spec, Some(dispatch))
            .await
    }

    /// Runs newest first, narrowed to a ticket or question, a kind or a session.
    pub async fn list(
        &self,
        subject: Option<RunSubject<'_>>,
        kind: Option<RunKind>,
        link: Option<RunLink>,
        limit: u32,
    ) -> Result<Vec<Run>, AppError> {
        let subject_key = match subject {
            Some(subject) => Some(normalized_subject(subject)?),
            None => None,
        };
        let filter = RunFilter {
            subject_key,
            kind,
            link,
        };
        self.runs.find(&filter, limit).await
    }

    pub async fn detail(&self, id: Id<Run>) -> Result<RunDetail, AppError> {
        let run = self.runs.get(id).await?;
        Ok(RunDetail {
            denied: self.runs.denied_tools(id).await?,
            artifacts: self.runs.artifacts(id).await?,
            run,
        })
    }

    /// Readable log lines of the output after `after`, at most `limit` output lines.
    pub async fn transcript(
        &self,
        id: Id<Run>,
        after: i64,
        limit: u32,
    ) -> Result<TranscriptPage, AppError> {
        let limit = limit.clamp(1, MAX_TRANSCRIPT_LINES);
        let (run, progress) = self.runs.output(id, after, limit).await?;
        let mut lines = progress.lines;
        let drained = read_whole_lines(&mut lines, limit, |from| async move {
            Ok(self.runs.output(id, from, limit).await?.1.lines)
        })
        .await?;
        let root = run.dispatch().map(|dispatch| dispatch.cwd.clone());
        // a lost run has no report but will not write more either
        let ended = progress.report.is_some() || !run.status().is_active();
        let finished = ended && drained;
        let _permit = FORMATTING.acquire().await.map_err(AppError::internal)?;
        // the lines can be huge, and parsing them must not hold the async runtime
        let (entries, last_seq) = tokio::task::spawn_blocking(move || {
            format_page(root.as_deref(), lines, after, finished, drained)
        })
        .await
        .map_err(AppError::internal)?;
        Ok(TranscriptPage {
            entries,
            last_seq,
            finished,
            more: !drained,
        })
    }

    /// The bytes of one screenshot and what is known about it.
    pub async fn screenshot(
        &self,
        id: Id<Run>,
        artifact: Uuid,
    ) -> Result<(RunArtifact, Vec<u8>), AppError> {
        self.runs.get(id).await?;
        let found = self
            .runs
            .artifacts(id)
            .await?
            .into_iter()
            .find(|candidate| candidate.id == artifact)
            .ok_or_else(|| artifact_not_found(id))?;
        let bytes = self
            .files
            .read_screenshot(id, &found.screenshot.file)
            .await?
            .ok_or_else(|| artifact_not_found(id))?;
        Ok((found, bytes))
    }
}

/// Whether pieces of two pipes leave a point where no line is half read.
fn has_line_end(lines: &[OutputLine]) -> bool {
    let mut open = [false; 2];
    lines.iter().any(|line| {
        open[usize::from(line.stream == OutputStream::Stderr)] = line.continues;
        !open[0] && !open[1]
    })
}

/// Reads on past the limit while the pieces hold no end of a line; true when the output is drained.
async fn read_whole_lines<F, Fut>(
    lines: &mut Vec<OutputLine>,
    limit: u32,
    mut read: F,
) -> Result<bool, AppError>
where
    F: FnMut(i64) -> Fut,
    Fut: Future<Output = Result<Vec<OutputLine>, AppError>>,
{
    let full = |read: usize| u32::try_from(read).is_ok_and(|read| read >= limit);
    let mut drained = !full(lines.len());
    while !drained && !has_line_end(lines) {
        let Some(last) = lines.last() else { break };
        let more = read(last.seq).await?;
        drained = !full(more.len());
        lines.extend(more);
    }
    Ok(drained)
}

fn emit(
    formatter: &mut TranscriptFormatter,
    entries: &mut Vec<TranscriptEntry>,
    seq: i64,
    pipe: Pipe,
    joined: Joined,
) {
    let found = match joined {
        Joined::Line(text) => formatter.format(pipe, &text),
        Joined::Dropped => vec![TranscriptLine::plain(LineKind::Raw, DROPPED_LINE)],
    };
    entries.extend(found.into_iter().map(|line| TranscriptEntry { seq, line }));
}

/// Joins the pieces of a page into lines and formats them; `last_seq` stops before a line cut off.
fn format_page(
    root: Option<&str>,
    lines: Vec<OutputLine>,
    after: i64,
    finished: bool,
    drained: bool,
) -> (Vec<TranscriptEntry>, i64) {
    let mut formatter = TranscriptFormatter::new(root);
    let mut joiners = [LineJoiner::default(), LineJoiner::default()];
    let mut entries = Vec::new();
    let mut last_seq = after;
    // the last point where no line was half read
    let mut safe = (after, 0);
    for line in lines {
        last_seq = line.seq;
        let (index, pipe) = match line.stream {
            OutputStream::Stdout => (0, Pipe::Stdout),
            OutputStream::Stderr => (1, Pipe::Stderr),
        };
        if let Some(joined) = joiners[index].push(&line.text, line.continues) {
            emit(&mut formatter, &mut entries, line.seq, pipe, joined);
        }
        if joiners.iter().all(LineJoiner::is_idle) {
            safe = (line.seq, entries.len());
        }
    }
    if joiners.iter().all(LineJoiner::is_idle) {
        return (entries, last_seq);
    }
    // more of a line may come on the next page, unless the output is over
    if !finished && (safe.0 != after || drained) {
        entries.truncate(safe.1);
        return (entries, safe.0);
    }
    for (index, pipe) in [(0, Pipe::Stdout), (1, Pipe::Stderr)] {
        if let Some(joined) = joiners[index].finish() {
            emit(&mut formatter, &mut entries, last_seq, pipe, joined);
        }
    }
    (entries, last_seq)
}

/// The canonical key of a ticket or question, given in any case.
fn normalized_subject(subject: RunSubject<'_>) -> Result<String, AppError> {
    match subject {
        RunSubject::Ticket(text) => TicketKey::parse(text)
            .map(|key| key.to_string())
            .map_err(|error| tickets::invalid(&error)),
        RunSubject::Question(text) => QuestionKey::parse(text)
            .map(|key| key.to_string())
            .map_err(|error| questions::invalid(&error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_normalized_whatever_the_case() {
        assert_eq!(
            normalized_subject(RunSubject::Ticket("arc-7")).unwrap(),
            "ARC-0007"
        );
        assert_eq!(
            normalized_subject(RunSubject::Question("arc-q7")).unwrap(),
            "ARC-Q0007"
        );
        assert_eq!(
            normalized_subject(RunSubject::Ticket("../x"))
                .unwrap_err()
                .code(),
            "ticket-id-invalid"
        );
        assert_eq!(
            normalized_subject(RunSubject::Ticket("ARC-Q0007"))
                .unwrap_err()
                .code(),
            "ticket-id-invalid"
        );
        assert_eq!(
            normalized_subject(RunSubject::Question(""))
                .unwrap_err()
                .code(),
            "question-id-invalid"
        );
    }

    fn piece(seq: i64, stream: OutputStream, text: &str, continues: bool) -> OutputLine {
        OutputLine {
            seq,
            stream,
            text: text.to_owned(),
            continues,
        }
    }

    #[test]
    fn a_line_in_pieces_is_one_transcript_entry() {
        let json =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello there"}]}}"#;
        let (head, tail) = json.split_at(30);
        let lines = vec![
            piece(1, OutputStream::Stdout, head, true),
            piece(2, OutputStream::Stderr, "boom", false),
            piece(3, OutputStream::Stdout, tail, false),
        ];
        let (entries, last) = format_page(None, lines, 0, true, true);
        let found: Vec<(i64, &str, &str)> = entries
            .iter()
            .map(|entry| {
                (
                    entry.seq,
                    entry.line.kind.as_str(),
                    entry.line.text.as_str(),
                )
            })
            .collect();
        assert_eq!(found, vec![(2, "raw", "boom"), (3, "say", "Hello there")]);
        assert_eq!(last, 3);
    }

    #[test]
    fn a_page_stops_before_a_line_that_goes_on_and_a_finished_one_flushes_it() {
        let lines = || {
            vec![
                piece(5, OutputStream::Stderr, "first", false),
                piece(6, OutputStream::Stdout, "{\"type\":", true),
            ]
        };
        let (entries, last) = format_page(None, lines(), 4, false, false);
        assert_eq!((entries.len(), last), (1, 5));
        let (entries, last) = format_page(None, lines(), 4, true, true);
        assert_eq!((entries.len(), last), (2, 6));
        // a page that holds only the start of a line gives nothing and keeps the cursor
        let (entries, last) = format_page(None, lines()[1..].to_vec(), 5, false, true);
        assert_eq!((entries.len(), last), (0, 5));
    }

    #[tokio::test]
    async fn a_small_limit_reads_on_until_the_line_ends() {
        let all = vec![
            piece(1, OutputStream::Stdout, "a", true),
            piece(2, OutputStream::Stdout, "b", true),
            piece(3, OutputStream::Stdout, "c", false),
            piece(4, OutputStream::Stdout, "next", false),
        ];
        let source = all.clone();
        let mut lines = vec![all[0].clone()];
        let drained = read_whole_lines(&mut lines, 1, |from| {
            let page: Vec<OutputLine> = source
                .iter()
                .filter(|line| line.seq > from)
                .take(1)
                .cloned()
                .collect();
            async move { Ok(page) }
        })
        .await
        .unwrap();
        assert_eq!(lines.len(), 3);
        assert!(!drained);
        let (entries, last) = format_page(None, lines, 0, false, drained);
        assert_eq!(
            (entries.len(), entries[0].line.text.as_str(), last),
            (1, "abc", 3)
        );
    }

    #[test]
    fn unknown_profiles_have_a_stable_error_code() {
        assert_eq!(profile_unknown("x").code(), "dispatch-profile-unknown");
    }
}
