//! What a session did, read on its runner from the transcript and the checkout.

use nullradix_domain::SessionClient;

use crate::AppError;
use crate::sessions::SessionLine;

/// Longest tool result in characters, as legacy.
pub const MAX_RESULT_CHARS: usize = 20_000;
/// Most diff lines per file, as legacy.
pub const MAX_DIFF_LINES: usize = 5_000;
/// Entries per transcript page without a limit.
pub const DEFAULT_PAGE_LIMIT: u32 = 500;
pub const MAX_PAGE_LIMIT: u32 = 2_000;
/// Note on changes read from the transcript.
pub const TRANSCRIPT_NOTE: &str = "Changes made through Bash are not included.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemLevel {
    Info,
    Warning,
    Error,
}

/// Output of a tool call, cut to [`MAX_RESULT_CHARS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
    pub truncated: bool,
}

/// One entry of the transcript; a later page may repeat an id to replace the entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    Prompt {
        id: String,
        at: String,
        text: String,
    },
    Text {
        id: String,
        at: String,
        text: String,
    },
    Thinking {
        id: String,
        at: String,
        text: String,
        duration_ms: Option<i64>,
    },
    Tool {
        id: String,
        at: String,
        name: String,
        summary: String,
        /// The tool input as JSON text.
        input: String,
        result: Option<ToolResult>,
    },
    System {
        id: String,
        at: String,
        text: String,
        level: SystemLevel,
    },
}

impl TranscriptEntry {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Prompt { id, .. }
            | Self::Text { id, .. }
            | Self::Thinking { id, .. }
            | Self::Tool { id, .. }
            | Self::System { id, .. } => id,
        }
    }
}

/// Entries from a byte offset on, with the offset of the next page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptChunk {
    pub entries: Vec<TranscriptEntry>,
    pub cursor: u64,
    /// Broken lines in this page.
    pub skipped: u64,
}

/// A page of the transcript at `path` on the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRead {
    pub path: String,
    pub after: u64,
    pub limit: u32,
}

/// A transcript and the folder its session works in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSource {
    pub path: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenTotals {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
}

/// Context use after the last answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsage {
    pub used_tokens: i64,
    pub limit_tokens: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
}

/// Figures of a session read from its transcript.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptFacts {
    pub client: Option<SessionClient>,
    pub branch: Option<String>,
    pub version: Option<String>,
    pub title: Option<String>,
    pub model: Option<String>,
    pub tokens: TokenTotals,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<i64>,
    pub context: Option<ContextUsage>,
    pub todos: Vec<TodoItem>,
}

/// Facts of a transcript with its size.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSummary {
    pub size_bytes: u64,
    pub facts: TranscriptFacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeOrigin {
    Git,
    Transcript,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// Relative to the checkout or the session folder, with `/`.
    pub path: String,
    pub status: FileStatus,
    pub added: i64,
    pub removed: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionChanges {
    pub origin: ChangeOrigin,
    /// Base branch when the changes come from git.
    pub base: Option<String>,
    pub note: Option<String>,
    pub files: Vec<ChangedFile>,
}

impl SessionChanges {
    #[must_use]
    pub const fn none() -> Self {
        Self {
            origin: ChangeOrigin::None,
            base: None,
            note: None,
            files: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Add,
    Remove,
    Context,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

/// A hunk with its header such as `@@ -1,3 +1,4 @@`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// Diff of one file, cut to [`MAX_DIFF_LINES`] lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub hunks: Vec<DiffHunk>,
    pub truncated: bool,
}

/// Where the runner looks for the changes of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRequest {
    pub cwd: String,
    /// The transcript, read when git has no changes of the session's own.
    pub transcript: Option<String>,
}

/// One file of [`ChangeRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRequest {
    pub changes: ChangeRequest,
    /// Relative to the checkout, checked by [`check_change_path`].
    pub path: String,
}

/// Where the transcript is and whether the runner could read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptInfo {
    pub available: bool,
    pub path: Option<String>,
    pub size_bytes: Option<u64>,
}

/// A session with the figures of its transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionDetail {
    pub line: SessionLine,
    /// The run, else the hooks, else the transcript.
    pub client: SessionClient,
    pub transcript: TranscriptInfo,
    pub facts: TranscriptFacts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptPage {
    pub entries: Vec<TranscriptEntry>,
    pub cursor: u64,
    pub skipped: u64,
    pub available: bool,
}

#[must_use]
pub fn change_not_found(path: &str) -> AppError {
    AppError::not_found(
        "change-not-found",
        format!("{path} is not among the changes of the session"),
    )
}

#[must_use]
pub fn change_path_invalid(path: &str) -> AppError {
    AppError::invalid(
        "change-path-invalid",
        format!("{path:?} is no path inside the checkout"),
    )
}

#[must_use]
pub fn transcript_path_invalid(path: &str) -> AppError {
    AppError::invalid(
        "transcript-path-invalid",
        format!("{path} is no absolute .jsonl file"),
    )
}

/// Refuses empty, absolute and option-like paths and any `..` part, as legacy.
pub fn check_change_path(path: &str) -> Result<(), AppError> {
    let normalized = path.replace('\\', "/");
    let invalid = normalized.trim().is_empty()
        || normalized.starts_with('/')
        || normalized.starts_with('-')
        || normalized.contains(':')
        || normalized.contains('\0')
        || normalized.split('/').any(|part| part == "..");
    if invalid {
        return Err(change_path_invalid(path));
    }
    Ok(())
}

/// Only an absolute `.jsonl` path without `..` parts may be read as a transcript.
pub fn check_transcript_path(path: &str) -> Result<(), AppError> {
    let normalized = path.replace('\\', "/");
    let bytes = normalized.as_bytes();
    let drive =
        bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/';
    let absolute = normalized.starts_with('/') || drive;
    let valid = absolute
        && normalized.to_ascii_lowercase().ends_with(".jsonl")
        && !normalized.contains('\0')
        && !normalized.split('/').any(|part| part == "..");
    if valid {
        Ok(())
    } else {
        Err(transcript_path_invalid(path))
    }
}

/// The run, else what the hooks reported, else the transcript, else unknown; as legacy.
#[must_use]
pub fn resolve_client(line: &SessionLine, facts: Option<&TranscriptFacts>) -> SessionClient {
    let record = line.session.record();
    if record.run_name.is_some() {
        return SessionClient::Nullradix;
    }
    if record.client != SessionClient::Unknown {
        return record.client;
    }
    facts
        .and_then(|facts| facts.client)
        .unwrap_or(SessionClient::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    #[test]
    fn change_paths_stay_inside_the_checkout() {
        for path in ["src/a.rs", "a b/c.txt", "src\\x.rs"] {
            assert!(check_change_path(path).is_ok(), "{path}");
        }
        for path in [
            "",
            "  ",
            "../x",
            "/etc/passwd",
            "C:/x",
            "-p",
            "a/../../b",
            "a\0b",
        ] {
            let error = check_change_path(path).unwrap_err();
            assert_eq!(error.code(), "change-path-invalid", "{path}");
            assert_eq!(error.kind(), ErrorKind::Invalid);
        }
        assert_eq!(change_not_found("a").kind(), ErrorKind::NotFound);
    }

    #[test]
    fn transcripts_are_absolute_jsonl_files() {
        for path in [
            "/home/a/.claude/projects/x/s.jsonl",
            "C:\\Users\\a\\s.JSONL",
            "D:/s.jsonl",
        ] {
            assert!(check_transcript_path(path).is_ok(), "{path}");
        }
        for path in [
            "s.jsonl",
            "/a/b.txt",
            "/a/../b.jsonl",
            "C:s.jsonl",
            "",
            "/a/b.jsonl\0",
        ] {
            assert_eq!(
                check_transcript_path(path).unwrap_err().code(),
                "transcript-path-invalid",
                "{path}"
            );
        }
    }
}
