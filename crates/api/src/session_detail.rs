//! What a session did: figures, transcript pages, changed files and diffs, and ticket claims.

use axum::Json;
use axum::extract::State;
use nullradix_app::session_detail::{
    ChangeOrigin, ChangedFile, DiffHunk, DiffLine, DiffLineKind, FileDiff, FileStatus,
    SessionChanges, SessionDetail, SystemLevel, TodoItem, TodoStatus, ToolResult, TranscriptEntry,
    TranscriptPage,
};
use nullradix_domain::session_label;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::sessions::{SessionClientDto, SessionDto, SessionPath};
use crate::{ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptInfoDto {
    /// Whether the runner could read the transcript the hooks named.
    pub available: bool,
    pub path: Option<String>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotalsDto {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
}

/// Context use after the last answer, input and cache included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageDto {
    pub used_tokens: i64,
    pub limit_tokens: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TodoStatusDto {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TodoItemDto {
    pub content: String,
    pub status: TodoStatusDto,
}

/// A session with the figures of its transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetailDto {
    /// The session as lists show it; model and title filled from the transcript when missing.
    pub session: SessionDto,
    /// The run, else the hooks, else the transcript.
    pub client: SessionClientDto,
    pub transcript: TranscriptInfoDto,
    /// Last branch the transcript names.
    pub branch: Option<String>,
    /// Claude Code version from the transcript.
    pub claude_version: Option<String>,
    pub tokens: TokenTotalsDto,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<i64>,
    pub context: Option<ContextUsageDto>,
    pub todos: Vec<TodoItemDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SystemLevelDto {
    Info,
    Warning,
    Error,
}

/// Output of a tool call, cut to 20000 characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultDto {
    pub text: String,
    pub is_error: bool,
    pub truncated: bool,
}

/// One transcript entry; a later page repeating an id replaces the entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TranscriptEntryDto {
    /// A prompt of the person.
    Prompt {
        id: String,
        at: String,
        text: String,
    },
    /// Answer text in Markdown.
    Text {
        id: String,
        at: String,
        text: String,
    },
    /// Thinking, often only its duration.
    Thinking {
        id: String,
        at: String,
        text: String,
        #[serde(rename = "durationMs")]
        duration_ms: Option<i64>,
    },
    /// A tool call with its summary, input and result once there.
    Tool {
        id: String,
        at: String,
        name: String,
        summary: String,
        /// The tool input as Claude Code sent it.
        input: serde_json::Value,
        result: Option<ToolResultDto>,
    },
    /// A note of the system such as a hook error or compaction.
    System {
        id: String,
        at: String,
        text: String,
        level: SystemLevelDto,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptPageDto {
    pub entries: Vec<TranscriptEntryDto>,
    /// Byte offset for `after` of the next page.
    pub cursor: u64,
    /// Broken lines in this page.
    pub skipped: u64,
    pub available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeOriginDto {
    Git,
    Transcript,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FileStatusDto {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFileDto {
    /// Relative to the checkout or the session folder, with `/`.
    pub path: String,
    pub status: FileStatusDto,
    pub added: i64,
    pub removed: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionChangesDto {
    pub origin: ChangeOriginDto,
    /// Base branch for changes from git.
    pub base: Option<String>,
    /// Such as that changes made through Bash are missing.
    pub note: Option<String>,
    pub files: Vec<ChangedFileDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DiffLineKindDto {
    Add,
    Remove,
    Context,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DiffLineDto {
    pub kind: DiffLineKindDto,
    /// The line without its prefix.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DiffHunkDto {
    /// Such as `@@ -1,3 +1,4 @@`.
    pub header: String,
    pub lines: Vec<DiffLineDto>,
}

/// Diff of one file, cut to 5000 lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FileDiffDto {
    pub path: String,
    pub hunks: Vec<DiffHunkDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaimSession {
    /// The ticket the session now works on.
    pub ticket_key: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct TranscriptQuery {
    /// Byte offset from the previous page's cursor; 0 starts at the beginning.
    after: Option<u64>,
    /// Entries per page, 500 by default and at most 2000.
    limit: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct FileQuery {
    /// Path of a changed file relative to the checkout.
    path: String,
}

impl From<TodoItem> for TodoItemDto {
    fn from(todo: TodoItem) -> Self {
        Self {
            content: todo.content,
            status: match todo.status {
                TodoStatus::Pending => TodoStatusDto::Pending,
                TodoStatus::InProgress => TodoStatusDto::InProgress,
                TodoStatus::Completed => TodoStatusDto::Completed,
            },
        }
    }
}

impl From<SessionDetail> for SessionDetailDto {
    fn from(detail: SessionDetail) -> Self {
        let facts = detail.facts;
        let mut session = SessionDto::from(detail.line);
        // what the hooks did not report, the transcript fills in
        if session.model.is_none() {
            session.model.clone_from(&facts.model);
        }
        if session.title.is_none() && facts.title.is_some() {
            session.title.clone_from(&facts.title);
            session.label = session_label(session.number, session.title.as_deref());
        }
        Self {
            session,
            client: detail.client.into(),
            transcript: TranscriptInfoDto {
                available: detail.transcript.available,
                path: detail.transcript.path,
                size_bytes: detail.transcript.size_bytes,
            },
            branch: facts.branch,
            claude_version: facts.version,
            tokens: TokenTotalsDto {
                input: facts.tokens.input,
                output: facts.tokens.output,
                cache_read: facts.tokens.cache_read,
                cache_write: facts.tokens.cache_write,
            },
            cost_usd: facts.cost_usd,
            duration_ms: facts.duration_ms,
            context: facts.context.map(|context| ContextUsageDto {
                used_tokens: context.used_tokens,
                limit_tokens: context.limit_tokens,
            }),
            todos: facts.todos.into_iter().map(TodoItemDto::from).collect(),
        }
    }
}

fn level(level: SystemLevel) -> SystemLevelDto {
    match level {
        SystemLevel::Info => SystemLevelDto::Info,
        SystemLevel::Warning => SystemLevelDto::Warning,
        SystemLevel::Error => SystemLevelDto::Error,
    }
}

fn tool_result(result: ToolResult) -> ToolResultDto {
    ToolResultDto {
        text: result.text,
        is_error: result.is_error,
        truncated: result.truncated,
    }
}

impl From<TranscriptEntry> for TranscriptEntryDto {
    fn from(entry: TranscriptEntry) -> Self {
        match entry {
            TranscriptEntry::Prompt { id, at, text } => Self::Prompt { id, at, text },
            TranscriptEntry::Text { id, at, text } => Self::Text { id, at, text },
            TranscriptEntry::Thinking {
                id,
                at,
                text,
                duration_ms,
            } => Self::Thinking {
                id,
                at,
                text,
                duration_ms,
            },
            TranscriptEntry::Tool {
                id,
                at,
                name,
                summary,
                input,
                result,
            } => Self::Tool {
                id,
                at,
                name,
                summary,
                input: serde_json::from_str(&input).unwrap_or(serde_json::Value::Null),
                result: result.map(tool_result),
            },
            TranscriptEntry::System {
                id,
                at,
                text,
                level: system,
            } => Self::System {
                id,
                at,
                text,
                level: level(system),
            },
        }
    }
}

impl From<TranscriptPage> for TranscriptPageDto {
    fn from(page: TranscriptPage) -> Self {
        Self {
            entries: page
                .entries
                .into_iter()
                .map(TranscriptEntryDto::from)
                .collect(),
            cursor: page.cursor,
            skipped: page.skipped,
            available: page.available,
        }
    }
}

impl From<ChangedFile> for ChangedFileDto {
    fn from(file: ChangedFile) -> Self {
        Self {
            path: file.path,
            status: match file.status {
                FileStatus::Added => FileStatusDto::Added,
                FileStatus::Modified => FileStatusDto::Modified,
                FileStatus::Deleted => FileStatusDto::Deleted,
                FileStatus::Renamed => FileStatusDto::Renamed,
            },
            added: file.added,
            removed: file.removed,
        }
    }
}

impl From<SessionChanges> for SessionChangesDto {
    fn from(changes: SessionChanges) -> Self {
        Self {
            origin: match changes.origin {
                ChangeOrigin::Git => ChangeOriginDto::Git,
                ChangeOrigin::Transcript => ChangeOriginDto::Transcript,
                ChangeOrigin::None => ChangeOriginDto::None,
            },
            base: changes.base,
            note: changes.note,
            files: changes
                .files
                .into_iter()
                .map(ChangedFileDto::from)
                .collect(),
        }
    }
}

fn line(line: DiffLine) -> DiffLineDto {
    DiffLineDto {
        kind: match line.kind {
            DiffLineKind::Add => DiffLineKindDto::Add,
            DiffLineKind::Remove => DiffLineKindDto::Remove,
            DiffLineKind::Context => DiffLineKindDto::Context,
        },
        text: line.text,
    }
}

fn hunk(hunk: DiffHunk) -> DiffHunkDto {
    DiffHunkDto {
        header: hunk.header,
        lines: hunk.lines.into_iter().map(line).collect(),
    }
}

impl From<FileDiff> for FileDiffDto {
    fn from(diff: FileDiff) -> Self {
        Self {
            path: diff.path,
            hunks: diff.hunks.into_iter().map(hunk).collect(),
            truncated: diff.truncated,
        }
    }
}

#[utoipa::path(
    get,
    path = "/sessions/{sessionId}/detail",
    tag = "sessions",
    params(SessionPath),
    responses(
        (status = 200, body = SessionDetailDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
        (status = 503, description = "The session's runner is not reachable", body = Problem),
    )
)]
pub(crate) async fn session_detail(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<SessionPath>,
) -> Result<Json<SessionDetailDto>, ApiError> {
    Ok(Json(state.sessions.detail(&path.session_id).await?.into()))
}

#[utoipa::path(
    get,
    path = "/sessions/{sessionId}/transcript",
    tag = "sessions",
    params(SessionPath, TranscriptQuery),
    responses(
        (status = 200, description = "A missing transcript is not available, no error", body = TranscriptPageDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
        (status = 503, body = Problem),
    )
)]
pub(crate) async fn session_transcript(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<SessionPath>,
    ApiQuery(query): ApiQuery<TranscriptQuery>,
) -> Result<Json<TranscriptPageDto>, ApiError> {
    let page = state
        .sessions
        .transcript(&path.session_id, query.after.unwrap_or(0), query.limit)
        .await?;
    Ok(Json(page.into()))
}

#[utoipa::path(
    get,
    path = "/sessions/{sessionId}/changes",
    tag = "sessions",
    params(SessionPath),
    responses(
        (status = 200, body = SessionChangesDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
        (status = 503, body = Problem),
    )
)]
pub(crate) async fn session_changes(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<SessionPath>,
) -> Result<Json<SessionChangesDto>, ApiError> {
    Ok(Json(state.sessions.changes(&path.session_id).await?.into()))
}

#[utoipa::path(
    get,
    path = "/sessions/{sessionId}/changes/file",
    tag = "sessions",
    params(SessionPath, FileQuery),
    responses(
        (status = 200, body = FileDiffDto),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown session or a file that did not change", body = Problem),
        (status = 422, description = "A path outside the checkout", body = Problem),
        (status = 503, body = Problem),
    )
)]
pub(crate) async fn session_file_diff(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<SessionPath>,
    ApiQuery(query): ApiQuery<FileQuery>,
) -> Result<Json<FileDiffDto>, ApiError> {
    let diff = state
        .sessions
        .file_diff(&path.session_id, &query.path)
        .await?;
    Ok(Json(diff.into()))
}

#[utoipa::path(
    post,
    path = "/sessions/{sessionId}/claim",
    tag = "sessions",
    params(SessionPath),
    request_body = ClaimSession,
    responses(
        (status = 200, body = SessionDto),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown session or ticket", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn claim_session(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<SessionPath>,
    ApiJson(input): ApiJson<ClaimSession>,
) -> Result<Json<SessionDto>, ApiError> {
    let line = state
        .sessions
        .claim(&path.session_id, &input.ticket_key)
        .await?;
    Ok(Json(line.into()))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{Value, json};

    use crate::tests::{TestApi, bearer, send, test_api};

    fn line(value: &Value) -> String {
        format!("{value}\n")
    }

    /// A prompt, a created file and the cost state, as Claude Code writes them.
    fn transcript(cwd: &str) -> String {
        let file = format!("{cwd}/new.txt");
        [
            json!({ "type": "custom-title", "customTitle": "From the transcript" }),
            json!({ "type": "user", "uuid": "u1", "timestamp": "t1", "entrypoint": "cli",
                    "gitBranch": "main", "message": { "content": "Add a file" } }),
            json!({ "type": "assistant", "uuid": "a1", "timestamp": "t2", "cwd": cwd,
                    "message": { "id": "m1", "model": "claude-x", "content": [
                        { "type": "tool_use", "id": "w1", "name": "Write",
                          "input": { "file_path": file, "content": "one\ntwo\n" } }
                    ], "usage": { "input_tokens": 3, "output_tokens": 4 } } }),
            json!({ "type": "user", "uuid": "u2", "timestamp": "t3",
                    "message": { "content": [
                        { "type": "tool_result", "tool_use_id": "w1", "content": "ok" }
                    ] }, "toolUseResult": { "type": "create" } }),
        ]
        .iter()
        .map(line)
        .collect()
    }

    async fn get(api: &TestApi, path: &str) -> (u16, Value) {
        let request = bearer(Request::get(path), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    async fn post(api: &TestApi, path: &str, body: &Value) -> (u16, Value) {
        let request = bearer(Request::post(path), &api.token)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    #[tokio::test]
    async fn the_detail_reads_the_transcript_the_hooks_named() {
        let api = test_api().await;
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().replace('\\', "/");
        let path = dir.path().join("s-1.jsonl");
        std::fs::write(&path, transcript(&cwd)).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let event = json!({
            "id": uuid::Uuid::now_v7().to_string(),
            "sessionId": "s-1",
            "cwd": cwd,
            "occurredAt": i64::try_from(now.as_millis()).unwrap(),
            "transcriptPath": path.to_string_lossy(),
        });
        let (status, reply) = post(&api, "/api/v1/hooks/session-start", &event).await;
        assert_eq!(status, 200, "{reply}");

        let (status, detail) = get(&api, "/api/v1/sessions/s-1/detail").await;
        assert_eq!(status, 200, "{detail}");
        assert_eq!(detail["transcript"]["available"], true);
        assert_eq!(detail["session"]["title"], "From the transcript");
        assert_eq!(detail["session"]["label"], "From the transcript");
        assert_eq!(detail["session"]["model"], "claude-x");
        assert_eq!(detail["client"], "claude-code");
        assert_eq!(detail["branch"], "main");
        assert_eq!(detail["tokens"]["output"], 4);

        let (status, page) = get(&api, "/api/v1/sessions/s-1/transcript").await;
        assert_eq!(status, 200, "{page}");
        let kinds: Vec<&str> = page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["prompt", "tool"]);
        assert_eq!(page["entries"][1]["summary"], "Write new.txt (+2)");
        assert_eq!(page["entries"][1]["input"]["content"], "one\ntwo\n");
        assert_eq!(page["entries"][1]["result"]["text"], "ok");
        let cursor = page["cursor"].as_u64().unwrap();
        assert_eq!(cursor, std::fs::metadata(&path).unwrap().len());
        let (_, rest) = get(
            &api,
            &format!("/api/v1/sessions/s-1/transcript?after={cursor}"),
        )
        .await;
        assert_eq!(rest["entries"], json!([]));

        let (status, changes) = get(&api, "/api/v1/sessions/s-1/changes").await;
        assert_eq!(status, 200, "{changes}");
        assert_eq!(changes["origin"], "transcript");
        assert_eq!(
            changes["files"],
            json!([{ "path": "new.txt", "status": "added", "added": 2, "removed": 0 }])
        );
        let (status, diff) = get(&api, "/api/v1/sessions/s-1/changes/file?path=new.txt").await;
        assert_eq!(status, 200, "{diff}");
        assert_eq!(diff["hunks"][0]["header"], "@@ -0,0 +1,2 @@");
        assert_eq!(
            diff["hunks"][0]["lines"][0],
            json!({ "kind": "add", "text": "one" })
        );
        let (status, problem) =
            get(&api, "/api/v1/sessions/s-1/changes/file?path=..%2Fsecret").await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("change-path-invalid"))
        );
        let (status, problem) = get(&api, "/api/v1/sessions/s-1/changes/file?path=old.txt").await;
        assert_eq!(
            (status, &problem["code"]),
            (404, &json!("change-not-found"))
        );
    }

    #[tokio::test]
    async fn claims_and_details_of_unknown_sessions_are_refused() {
        let api = test_api().await;
        let (status, problem) = get(&api, "/api/v1/sessions/nope/detail").await;
        assert_eq!(
            (status, &problem["code"]),
            (404, &json!("session-not-found"))
        );
        let (status, problem) = get(&api, "/api/v1/sessions/nope/transcript?after=x").await;
        assert_eq!(status, 422, "{problem}");
        let (status, problem) = post(
            &api,
            "/api/v1/sessions/nope/claim",
            &json!({ "ticketKey": "ARC-1" }),
        )
        .await;
        assert_eq!(
            (status, &problem["code"]),
            (404, &json!("session-not-found"))
        );
    }
}
