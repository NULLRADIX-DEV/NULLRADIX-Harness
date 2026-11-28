use std::net::SocketAddr;
use std::time::Duration;

use nullradix_api::{
    API_PREFIX, API_VERSION, AcceptTicket, AccountDto, AccountList, AccountLoginDto,
    AccountStatusList, AddCatalogSource, AddComment, AddQuestionComment, AddStep, AnswerQuestion,
    BoardDeleted, BoardDto, BoardList, CatalogDto, CatalogSourceDto, CatalogSourceListDto,
    ChangeStep, ChooseTheme, ClaimPlanning, ClaimSession, ClaimTicket, CommentDto, CommentList,
    ConfirmationDto, CreateAccount, CreateAccountLogin, CreateBoard, CreateCustomization,
    CreateNote, CreateProject, CreateQuestion, CreateTicket, CustomizationDto, CustomizationList,
    DecideQuestion, DispatcherConfigView, DispatcherStatusDto, DropNote, EffectiveDto,
    ExtensionPointDto, FavoriteDto, FavoriteList, FileDiffDto, FinishTicket, FinishedTicket,
    Handoff, Health, HookEventDto, HookEventList, HookKindDto, HookReplyDto, InboxList, LoginCode,
    LoginDto, MarkFavorite, MergeRetryDto, MoveStep, MoveTicket, NextPlanning, NextPlanningClaim,
    NoteDto, NoteList, PassReportDto, PauseDispatcher, PreviewPrompt, Problem, ProjectDto,
    ProjectList, PromoteNote, PromotedNote, PromptDetail, PromptKindDto, PromptListDto,
    PromptPreviewDto, QuestionCommentDto, QuestionDetailDto, QuestionList, RUN_HEADER,
    RebaseCustomization, ReleaseTicket, RenameBoard, RenameProject, RepositoryList, RetryMerge,
    ReturnReview, RunDto, RunList, RunOutput, RunStatusDto, RunnerList, SESSION_HEADER,
    SaveThemeSettings, SeenNote, SessionChangesDto, SessionDetailDto, SessionDto, SessionList,
    SetBoardRepository, SetPrompt, SetRunToken, StartLogin, StartRun, StartTicket, StartedTicket,
    StepUpCode, SubmitResearch, ThemeDto, ThemeListDto, ThemeSettingsDto, TicketDetailDto,
    TicketDto, TicketList, TranscriptPageDto, TurnOffCode, UpdateAccount, UpdateCustomization,
    UpdateDispatcher, UpdateDispatcherConfig, UpdateTicket, WritePlan,
};
use nullradix_app::auth::Secret;
use serde::de::DeserializeOwned;
use ureq::http::{HeaderValue, Request, Response};
use ureq::middleware::{Middleware, MiddlewareNext};
use ureq::{Agent, Body, SendBody};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("the daemon is not reachable at {address}")]
    Unreachable {
        address: SocketAddr,
        #[source]
        source: Box<ureq::Error>,
    },
    #[error("the daemon speaks API version {found}, this client needs {expected}")]
    VersionMismatch { found: u32, expected: u32 },
    #[error("no local token, start the daemon once to create it")]
    MissingToken,
    #[error("the daemon refused the request: {detail} ({code})")]
    Refused {
        status: u16,
        code: String,
        detail: String,
    },
    #[error("the daemon refused the request: {detail} (theme-confirmation-required)")]
    ConfirmationRequired {
        detail: String,
        facts: Box<ConfirmationDto>,
    },
    #[error("unexpected response from the daemon: {0}")]
    Protocol(String),
}

/// Longer than the daemon's own limit for downloading from a catalog.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(150);

/// Tries of one GET when the connection is refused, reset or times out while connecting.
const GET_ATTEMPTS: u32 = 4;
const GET_PAUSE: Duration = Duration::from_millis(100);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);
/// The most runs the daemon lists at once; a run that has not ended is among the newest.
const ACTIVE_RUN_LOOKBACK: u32 = 500;

/// Whether the connection failed or dropped; only safe to repeat for reads.
fn is_transient(error: &ureq::Error) -> bool {
    use std::io::ErrorKind;
    match error {
        ureq::Error::ConnectionFailed | ureq::Error::Timeout(ureq::Timeout::Connect) => true,
        ureq::Error::Io(io) => matches!(
            io.kind(),
            ErrorKind::ConnectionReset
                | ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionAborted
                | ErrorKind::BrokenPipe
                | ErrorKind::UnexpectedEof
        ),
        _ => false,
    }
}

/// The run and session of an agent the caller works in; the daemon refuses its decisions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentContext {
    pub run: Option<String>,
    pub session: Option<String>,
}

impl AgentContext {
    /// The context named by `NULLRADIX_RUN` and `NULLRADIX_SESSION`; an empty value counts as none.
    #[must_use]
    pub fn from_env() -> Self {
        let read = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Self {
            run: read("NULLRADIX_RUN"),
            session: read("NULLRADIX_SESSION"),
        }
    }
}

/// Adds the headers that tell the daemon a request comes from an agent.
struct AgentHeaders(AgentContext);

impl Middleware for AgentHeaders {
    fn handle(
        &self,
        mut request: Request<SendBody<'_>>,
        next: MiddlewareNext<'_>,
    ) -> Result<Response<Body>, ureq::Error> {
        for (name, value) in [(RUN_HEADER, &self.0.run), (SESSION_HEADER, &self.0.session)] {
            if let Some(value) = value {
                // the header counts by being there, so an odd value still goes out
                let value = HeaderValue::from_str(value)
                    .unwrap_or_else(|_| HeaderValue::from_static("agent"));
                request.headers_mut().insert(name, value);
            }
        }
        next.handle(request)
    }
}

fn build_agent(
    connect_timeout: Option<Duration>,
    timeout: Duration,
    context: &AgentContext,
) -> Agent {
    let builder = Agent::config_builder()
        .timeout_connect(connect_timeout)
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .proxy(None);
    if *context == AgentContext::default() {
        builder.build().into()
    } else {
        builder
            .middleware(AgentHeaders(context.clone()))
            .build()
            .into()
    }
}

/// Blocking HTTP client for short-lived callers such as the CLI and hooks.
#[derive(Debug, Clone)]
pub struct Client {
    agent: Agent,
    address: SocketAddr,
    token: Option<Secret>,
    connect_timeout: Option<Duration>,
    timeout: Duration,
    context: AgentContext,
}

impl Client {
    #[must_use]
    pub fn new(address: SocketAddr, connect_timeout: Duration, timeout: Duration) -> Self {
        let context = AgentContext::default();
        Self {
            agent: build_agent(Some(connect_timeout), timeout, &context),
            address,
            token: None,
            connect_timeout: Some(connect_timeout),
            timeout,
            context,
        }
    }

    /// The same client that names the run and session of an agent on every request.
    #[must_use]
    pub fn with_context(mut self, context: AgentContext) -> Self {
        self.agent = build_agent(self.connect_timeout, self.timeout, &context);
        self.context = context;
        self
    }

    #[must_use]
    pub fn with_token(mut self, token: Secret) -> Self {
        self.token = Some(token);
        self
    }

    /// The same client with another limit for a whole request.
    #[must_use]
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        Self {
            agent: build_agent(self.connect_timeout, timeout, &self.context),
            address: self.address,
            token: self.token.clone(),
            connect_timeout: self.connect_timeout,
            timeout,
            context: self.context.clone(),
        }
    }

    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn health(&self) -> Result<Health, ClientError> {
        // a daemon that accepts but never answers is reported soon, not after the long limit
        let limit = self.timeout.min(HEALTH_TIMEOUT);
        let mut response = self.get_within(&self.url("/health"), None, Some(limit))?;
        decode(&mut response, 200)
    }

    /// Health check that also refuses a daemon with another API major version.
    pub fn handshake(&self) -> Result<Health, ClientError> {
        let health = self.health()?;
        if health.api_version != API_VERSION {
            return Err(ClientError::VersionMismatch {
                found: health.api_version,
                expected: API_VERSION,
            });
        }
        Ok(health)
    }

    /// Asks the daemon to exit; the answer comes before it is gone, and its runs go on.
    pub fn stop_daemon(&self) -> Result<(), ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/daemon/stop"))
            .header("Authorization", &self.authorization()?)
            .send_empty()
            .map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 202)
    }

    /// One-time code that opens a browser session.
    pub fn create_handoff(&self) -> Result<Handoff, ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/auth/handoffs"))
            .header("Authorization", &self.authorization()?)
            .send_empty()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    /// One-time code to confirm a browser session for command changes.
    pub fn create_step_up_code(&self) -> Result<StepUpCode, ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/auth/step-up-codes"))
            .header("Authorization", &self.authorization()?)
            .send_empty()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    /// One-time code for turning the confirmation off, never part of a link or menu.
    pub fn create_turn_off_code(&self) -> Result<TurnOffCode, ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/auth/turn-off-codes"))
            .header("Authorization", &self.authorization()?)
            .send_empty()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    pub fn list_projects(&self) -> Result<ProjectList, ClientError> {
        self.get_json("/projects")
    }

    pub fn create_project(&self, input: &CreateProject) -> Result<ProjectDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/projects"))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    pub fn rename_project(
        &self,
        id: Uuid,
        input: &RenameProject,
    ) -> Result<ProjectDto, ClientError> {
        let mut response = self
            .agent
            .patch(&self.url(&format!("/projects/{id}")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn delete_project(&self, id: Uuid, version: i64) -> Result<(), ClientError> {
        let mut response = self
            .agent
            .delete(&self.url(&format!("/projects/{id}?version={version}")))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 204)
    }

    pub fn list_accounts(&self) -> Result<AccountList, ClientError> {
        self.get_json("/accounts")
    }

    /// Login and limits of every account; `fresh` asks Anthropic again.
    pub fn account_usage(&self, fresh: bool) -> Result<AccountStatusList, ClientError> {
        self.get_json(&format!("/accounts/usage?fresh={fresh}"))
    }

    pub fn dispatcher_status(&self) -> Result<DispatcherStatusDto, ClientError> {
        self.get_json("/dispatcher")
    }

    pub fn set_dispatcher_enabled(
        &self,
        enabled: bool,
    ) -> Result<DispatcherStatusDto, ClientError> {
        let mut response = self
            .agent
            .patch(&self.url("/dispatcher"))
            .header("Authorization", &self.authorization()?)
            .send_json(UpdateDispatcher { enabled })
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn pause_dispatcher(
        &self,
        input: &PauseDispatcher,
    ) -> Result<DispatcherStatusDto, ClientError> {
        self.post_json("/dispatcher/pause", input, 200)
    }

    pub fn resume_dispatcher(&self) -> Result<DispatcherStatusDto, ClientError> {
        self.post_empty("/dispatcher/resume", 200)
    }

    /// Runs one pass now and returns the state after it.
    pub fn run_dispatcher_pass(&self) -> Result<PassReportDto, ClientError> {
        self.post_empty("/dispatcher/passes", 200)
    }

    pub fn dispatcher_config(&self) -> Result<DispatcherConfigView, ClientError> {
        self.get_json("/dispatcher/config")
    }

    pub fn dispatcher_prompts(&self) -> Result<PromptListDto, ClientError> {
        self.get_json("/dispatcher/prompts")
    }

    pub fn dispatcher_prompt(&self, kind: PromptKindDto) -> Result<PromptDetail, ClientError> {
        self.get_json(&format!("/dispatcher/prompts/{}", kind.as_str()))
    }

    pub fn set_dispatcher_prompt(
        &self,
        kind: PromptKindDto,
        input: &SetPrompt,
    ) -> Result<PromptDetail, ClientError> {
        let mut response = self
            .agent
            .put(&self.url(&format!("/dispatcher/prompts/{}", kind.as_str())))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn reset_dispatcher_prompt(
        &self,
        kind: PromptKindDto,
        version: i64,
    ) -> Result<PromptDetail, ClientError> {
        let route = format!("/dispatcher/prompts/{}?version={version}", kind.as_str());
        let mut response = self
            .agent
            .delete(&self.url(&route))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn preview_dispatcher_prompt(
        &self,
        input: &PreviewPrompt,
    ) -> Result<PromptPreviewDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/dispatcher/prompt-previews"))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn update_dispatcher_config(
        &self,
        input: &UpdateDispatcherConfig,
    ) -> Result<DispatcherConfigView, ClientError> {
        let mut response = self
            .agent
            .put(&self.url("/dispatcher/config"))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn create_account(&self, input: &CreateAccount) -> Result<AccountDto, ClientError> {
        self.post_json("/accounts", input, 201)
    }

    pub fn update_account(
        &self,
        id: Uuid,
        input: &UpdateAccount,
    ) -> Result<AccountDto, ClientError> {
        let mut response = self
            .agent
            .patch(&self.url(&format!("/accounts/{id}")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn delete_account(
        &self,
        id: Uuid,
        version: i64,
        delete_dir: bool,
    ) -> Result<(), ClientError> {
        let mut response = self
            .agent
            .delete(&self.url(&format!(
                "/accounts/{id}?version={version}&deleteDir={delete_dir}"
            )))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 204)
    }

    pub fn set_run_token(&self, id: Uuid, input: &SetRunToken) -> Result<AccountDto, ClientError> {
        let mut response = self
            .agent
            .put(&self.url(&format!("/accounts/{id}/run-token")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Lets the dispatcher use the account again before its pause ends.
    pub fn resume_account(&self, id: Uuid) -> Result<AccountDto, ClientError> {
        self.post_empty(&format!("/accounts/{id}/resume"), 200)
    }

    pub fn clear_run_token(&self, id: Uuid, version: i64) -> Result<AccountDto, ClientError> {
        let mut response = self
            .agent
            .delete(&self.url(&format!("/accounts/{id}/run-token?version={version}")))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn create_account_login(
        &self,
        input: &CreateAccountLogin,
    ) -> Result<AccountLoginDto, ClientError> {
        self.post_json("/accounts/login", input, 202)
    }

    pub fn start_login(&self, id: Uuid, input: &StartLogin) -> Result<LoginDto, ClientError> {
        self.post_json(&format!("/accounts/{id}/login"), input, 202)
    }

    pub fn get_login(&self, id: Uuid) -> Result<LoginDto, ClientError> {
        self.get_json(&format!("/accounts/{id}/login"))
    }

    pub fn cancel_login(&self, id: Uuid) -> Result<(), ClientError> {
        let mut response = self
            .agent
            .delete(&self.url(&format!("/accounts/{id}/login")))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 204)
    }

    pub fn send_login_code(&self, id: Uuid, input: &LoginCode) -> Result<(), ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/accounts/{id}/login/code")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 204)
    }

    pub fn list_all_boards(&self) -> Result<BoardList, ClientError> {
        self.get_json("/boards")
    }

    pub fn list_boards(&self, project: Uuid) -> Result<BoardList, ClientError> {
        self.get_json(&format!("/projects/{project}/boards"))
    }

    /// Boards of the repository that `path` on `runner` belongs to.
    pub fn list_boards_at(&self, runner: Uuid, path: &str) -> Result<BoardList, ClientError> {
        let mut response = self
            .agent
            .get(&self.url("/boards"))
            .header("Authorization", &self.authorization()?)
            .query("runnerId", runner.to_string())
            .query("path", path)
            .call()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn get_board(&self, project: Uuid, board: Uuid) -> Result<BoardDto, ClientError> {
        self.get_json(&format!("/projects/{project}/boards/{board}"))
    }

    pub fn create_board(
        &self,
        project: Uuid,
        input: &CreateBoard,
    ) -> Result<BoardDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/projects/{project}/boards")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    pub fn rename_board(
        &self,
        project: Uuid,
        board: Uuid,
        input: &RenameBoard,
    ) -> Result<BoardDto, ClientError> {
        let mut response = self
            .agent
            .patch(&self.url(&format!("/projects/{project}/boards/{board}")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Deletes the board with its tickets; open ones only with `delete_open`.
    pub fn delete_board(
        &self,
        project: Uuid,
        board: Uuid,
        version: i64,
        delete_open: bool,
    ) -> Result<BoardDeleted, ClientError> {
        let mut response = self
            .agent
            .delete(&self.url(&format!(
                "/projects/{project}/boards/{board}?version={version}\
                 &deleteOpenTickets={delete_open}"
            )))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Points the board to the repository checked out at a folder on a runner.
    pub fn set_board_repository(
        &self,
        project: Uuid,
        board: Uuid,
        input: &SetBoardRepository,
    ) -> Result<BoardDto, ClientError> {
        let mut response = self
            .agent
            .put(&self.url(&format!("/projects/{project}/boards/{board}/repository")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn clear_board_repository(
        &self,
        project: Uuid,
        board: Uuid,
        version: i64,
    ) -> Result<BoardDto, ClientError> {
        let mut response = self
            .agent
            .delete(&self.url(&format!(
                "/projects/{project}/boards/{board}/repository?version={version}"
            )))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn list_runners(&self) -> Result<RunnerList, ClientError> {
        self.get_json("/runners")
    }

    pub fn list_repositories(&self) -> Result<RepositoryList, ClientError> {
        self.get_json("/repositories")
    }

    pub fn list_runs(&self, limit: u32) -> Result<RunList, ClientError> {
        self.get_json(&format!("/runs?limit={limit}"))
    }

    /// How many runs have not ended; their supervisors go on without the daemon.
    pub fn active_run_count(&self) -> Result<usize, ClientError> {
        let runs = self.list_runs(ACTIVE_RUN_LOOKBACK)?;
        Ok(runs
            .items
            .iter()
            .filter(|run| matches!(run.status, RunStatusDto::Starting | RunStatusDto::Running))
            .count())
    }

    pub fn start_run(&self, input: &StartRun) -> Result<RunDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/runs"))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    pub fn get_run(&self, id: Uuid) -> Result<RunDto, ClientError> {
        self.get_json(&format!("/runs/{id}"))
    }

    pub fn run_output(&self, id: Uuid, after: i64) -> Result<RunOutput, ClientError> {
        self.get_json(&format!("/runs/{id}/output?after={after}"))
    }

    pub fn cancel_run(&self, id: Uuid) -> Result<RunDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/runs/{id}/cancel")))
            .header("Authorization", &self.authorization()?)
            .send_empty()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 202)
    }

    pub fn post_hook(
        &self,
        kind: HookKindDto,
        event: &HookEventDto,
    ) -> Result<HookReplyDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/hooks/{}", kind.as_str())))
            .header("Authorization", &self.authorization()?)
            .send_json(event)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Sessions matching `query`, given as pairs such as `("state", "running")`.
    pub fn list_sessions(&self, query: &[(&str, &str)]) -> Result<SessionList, ClientError> {
        let mut request = self
            .agent
            .get(&self.url("/sessions"))
            .header("Authorization", &self.authorization()?);
        for (name, value) in query {
            request = request.query(*name, *value);
        }
        let mut response = request.call().map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn get_session(&self, id: &str) -> Result<SessionDto, ClientError> {
        self.get_json(&format!("/sessions/{}", segment(id)))
    }

    pub fn session_detail(&self, id: &str) -> Result<SessionDetailDto, ClientError> {
        self.get_json(&format!("/sessions/{}/detail", segment(id)))
    }

    /// Transcript entries after byte `after`, at most `limit` when given.
    pub fn session_transcript(
        &self,
        id: &str,
        after: u64,
        limit: Option<u32>,
    ) -> Result<TranscriptPageDto, ClientError> {
        let mut request = self
            .agent
            .get(&self.url(&format!("/sessions/{}/transcript", segment(id))))
            .header("Authorization", &self.authorization()?)
            .query("after", after.to_string());
        if let Some(limit) = limit {
            request = request.query("limit", limit.to_string());
        }
        let mut response = request.call().map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn session_changes(&self, id: &str) -> Result<SessionChangesDto, ClientError> {
        self.get_json(&format!("/sessions/{}/changes", segment(id)))
    }

    pub fn session_diff(&self, id: &str, path: &str) -> Result<FileDiffDto, ClientError> {
        let mut response = self
            .agent
            .get(&self.url(&format!("/sessions/{}/changes/file", segment(id))))
            .header("Authorization", &self.authorization()?)
            .query("path", path)
            .call()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Installed themes and the chosen one.
    pub fn list_themes(&self) -> Result<ThemeListDto, ClientError> {
        self.get_json("/themes")
    }

    /// Installs a package or replaces the theme; the daemon refuses what the person must confirm.
    pub fn install_theme(&self, package: &[u8]) -> Result<ThemeDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/themes"))
            .header("Authorization", &self.authorization()?)
            .header("Content-Type", "application/octet-stream")
            .send(package)
            .map_err(|source| self.unreachable(source))?;
        let expected = if response.status().as_u16() == 200 {
            200
        } else {
            201
        };
        decode(&mut response, expected)
    }

    /// The addresses the person added to look for themes.
    pub fn catalog_sources(&self) -> Result<CatalogSourceListDto, ClientError> {
        self.get_json("/theme-catalog/sources")
    }

    pub fn add_catalog_source(&self, url: &str) -> Result<CatalogSourceDto, ClientError> {
        let input = AddCatalogSource {
            url: url.to_owned(),
        };
        self.post_json("/theme-catalog/sources", &input, 201)
    }

    pub fn remove_catalog_source(&self, id: &str, version: i64) -> Result<(), ClientError> {
        let route = format!("/theme-catalog/sources/{}?version={version}", segment(id));
        let mut response = self
            .agent
            .delete(&self.url(&route))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 204)
    }

    /// The themes of every source; the daemon downloads the indexes, which can take a while.
    pub fn browse_catalog(&self) -> Result<CatalogDto, ClientError> {
        let mut response = self
            .agent
            .get(&self.url("/theme-catalog/themes"))
            .config()
            .timeout_global(Some(DOWNLOAD_TIMEOUT))
            .build()
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Installs a theme of a source; the daemon refuses what the person must confirm.
    pub fn install_from_catalog(
        &self,
        source: &str,
        theme: &str,
        replace: bool,
    ) -> Result<ThemeDto, ClientError> {
        let query = if replace { "?replace=true" } else { "" };
        let route = format!(
            "/theme-catalog/sources/{}/themes/{}/install{query}",
            segment(source),
            segment(theme)
        );
        let mut response = self
            .agent
            .post(&self.url(&route))
            .config()
            .timeout_global(Some(DOWNLOAD_TIMEOUT))
            .build()
            .header("Authorization", &self.authorization()?)
            .send_empty()
            .map_err(|source| self.unreachable(source))?;
        let expected = if response.status().as_u16() == 200 {
            200
        } else {
            201
        };
        decode(&mut response, expected)
    }

    /// Whether the Claude part of the chosen theme is put into the Claude folders, and where.
    pub fn claude_theme(&self) -> Result<nullradix_api::ClaudeThemeDto, ClientError> {
        self.get_json("/settings/claude-theme")
    }

    /// Puts the Claude part of the chosen theme into the Claude folders, or takes it out.
    pub fn set_claude_theme(
        &self,
        enabled: bool,
    ) -> Result<nullradix_api::ClaudeThemeDto, ClientError> {
        let input = nullradix_api::SetClaudeTheme { enabled };
        let mut response = self
            .agent
            .put(&self.url("/settings/claude-theme"))
            .header("Authorization", &self.authorization()?)
            .send_json(&input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Chooses a built-in or an installed theme.
    pub fn choose_theme(&self, theme: &str) -> Result<ChooseTheme, ClientError> {
        let input = ChooseTheme {
            theme: theme.to_owned(),
        };
        let mut response = self
            .agent
            .put(&self.url("/settings/theme"))
            .header("Authorization", &self.authorization()?)
            .send_json(&input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn remove_theme(&self, id: &str, version: i64) -> Result<(), ClientError> {
        let mut response = self
            .agent
            .delete(&self.url(&format!("/themes/{}?version={version}", segment(id))))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 204)
    }

    /// The values chosen for the settings of an installed theme.
    pub fn theme_settings(&self, id: &str) -> Result<ThemeSettingsDto, ClientError> {
        self.get_json(&format!("/themes/{}/settings", segment(id)))
    }

    /// Replaces the chosen values; `input.version` is 0 while nothing was saved.
    pub fn save_theme_settings(
        &self,
        id: &str,
        input: &SaveThemeSettings,
    ) -> Result<ThemeSettingsDto, ClientError> {
        let mut response = self
            .agent
            .put(&self.url(&format!("/themes/{}/settings", segment(id))))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Back to the defaults of the manifest.
    pub fn reset_theme_settings(&self, id: &str, version: i64) -> Result<(), ClientError> {
        let route = format!("/themes/{}/settings?version={version}", segment(id));
        let mut response = self
            .agent
            .delete(&self.url(&route))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 204)
    }

    /// Customizations, skills first, each by name.
    pub fn list_customizations(&self) -> Result<CustomizationList, ClientError> {
        self.get_json("/customizations")
    }

    pub fn get_customization(
        &self,
        point: ExtensionPointDto,
        name: &str,
    ) -> Result<CustomizationDto, ClientError> {
        self.get_json(&customization_route(point, name))
    }

    /// Starts a customization from `content`, or from the shipped file when none is given.
    pub fn create_customization(
        &self,
        input: &CreateCustomization,
    ) -> Result<CustomizationDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/customizations"))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    pub fn update_customization(
        &self,
        point: ExtensionPointDto,
        name: &str,
        input: &UpdateCustomization,
    ) -> Result<CustomizationDto, ClientError> {
        let mut response = self
            .agent
            .patch(&self.url(&customization_route(point, name)))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn rebase_customization(
        &self,
        point: ExtensionPointDto,
        name: &str,
        version: i64,
    ) -> Result<CustomizationDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("{}/rebase", customization_route(point, name))))
            .header("Authorization", &self.authorization()?)
            .send_json(&RebaseCustomization { version })
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn delete_customization(
        &self,
        point: ExtensionPointDto,
        name: &str,
        version: i64,
    ) -> Result<(), ClientError> {
        let route = format!("{}?version={version}", customization_route(point, name));
        let mut response = self
            .agent
            .delete(&self.url(&route))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 204)
    }

    /// The customization when it applies, else the shipped file.
    pub fn effective_customization(
        &self,
        point: ExtensionPointDto,
        name: &str,
    ) -> Result<EffectiveDto, ClientError> {
        self.get_json(&format!("{}/effective", customization_route(point, name)))
    }

    /// Favorites, latest marked first; only those of `repository` when given.
    pub fn list_favorites(&self, repository: Option<&str>) -> Result<FavoriteList, ClientError> {
        let mut request = self
            .agent
            .get(&self.url("/favorites"))
            .header("Authorization", &self.authorization()?);
        if let Some(repository) = repository {
            request = request.query("repositoryId", repository);
        }
        let mut response = request.call().map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Marks the session; a note replaces the old one, none keeps it.
    pub fn mark_favorite(&self, id: &str, note: Option<&str>) -> Result<FavoriteDto, ClientError> {
        let input = MarkFavorite {
            note: note.map(str::to_owned),
        };
        let mut response = self
            .agent
            .put(&self.url(&format!("/sessions/{}/favorite", segment(id))))
            .header("Authorization", &self.authorization()?)
            .send_json(&input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn unmark_favorite(&self, id: &str) -> Result<(), ClientError> {
        let mut response = self
            .agent
            .delete(&self.url(&format!("/sessions/{}/favorite", segment(id))))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 204)
    }

    /// Links the ticket to the session, as a session taking a ticket does.
    pub fn claim_session(&self, id: &str, ticket_key: &str) -> Result<SessionDto, ClientError> {
        let input = ClaimSession {
            ticket_key: ticket_key.to_owned(),
        };
        self.post_json(&format!("/sessions/{}/claim", segment(id)), &input, 200)
    }

    /// Tickets matching `query`, given as pairs such as `("columns", "planned")`.
    pub fn list_tickets(&self, query: &[(&str, &str)]) -> Result<TicketList, ClientError> {
        let mut request = self
            .agent
            .get(&self.url("/tickets"))
            .header("Authorization", &self.authorization()?);
        for (name, value) in query {
            request = request.query(*name, *value);
        }
        let mut response = request.call().map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn create_ticket(
        &self,
        project: Uuid,
        board: Uuid,
        input: &CreateTicket,
    ) -> Result<TicketDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/projects/{project}/boards/{board}/tickets")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    /// `key` must be a ticket key such as `ARC-0007`; it goes into the path as is.
    pub fn get_ticket(&self, key: &str) -> Result<TicketDetailDto, ClientError> {
        self.get_json(&format!("/tickets/{key}"))
    }

    /// `key` must be a ticket key such as `ARC-0007`, as for [`Client::get_ticket`].
    pub fn update_ticket(
        &self,
        key: &str,
        input: &UpdateTicket,
    ) -> Result<TicketDetailDto, ClientError> {
        let mut response = self
            .agent
            .patch(&self.url(&format!("/tickets/{key}")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn move_ticket(&self, key: &str, input: &MoveTicket) -> Result<TicketDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/tickets/{key}/move")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Comments of the ticket, oldest first; `key` as for [`Client::get_ticket`].
    pub fn list_comments(&self, key: &str) -> Result<CommentList, ClientError> {
        self.get_json(&format!("/tickets/{key}/comments"))
    }

    pub fn add_comment(&self, key: &str, input: &AddComment) -> Result<CommentDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/tickets/{key}/comments")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    /// Questions of the project matching `query`, such as `("state", "open")`.
    pub fn list_questions(
        &self,
        project: Uuid,
        query: &[(&str, &str)],
    ) -> Result<QuestionList, ClientError> {
        let mut request = self
            .agent
            .get(&self.url(&format!("/projects/{project}/questions")))
            .header("Authorization", &self.authorization()?);
        for (name, value) in query {
            request = request.query(*name, *value);
        }
        let mut response = request.call().map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn create_question(
        &self,
        project: Uuid,
        input: &CreateQuestion,
    ) -> Result<QuestionDetailDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/projects/{project}/questions")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    /// `key` must be a question key such as `ARC-Q0007`; it goes into the path as is.
    pub fn get_question(&self, key: &str) -> Result<QuestionDetailDto, ClientError> {
        self.get_json(&format!("/questions/{key}"))
    }

    pub fn answer_question(
        &self,
        key: &str,
        input: &AnswerQuestion,
    ) -> Result<QuestionDetailDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/questions/{key}/answer")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Discards or reopens the question; `step` is `discard` or `reopen`.
    pub fn decide_question(
        &self,
        key: &str,
        step: &str,
        input: &DecideQuestion,
    ) -> Result<QuestionDetailDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/questions/{key}/{step}")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Appends a round of context and options to the question.
    pub fn submit_research(
        &self,
        key: &str,
        input: &SubmitResearch,
    ) -> Result<QuestionDetailDto, ClientError> {
        self.post_json(&format!("/questions/{key}/research"), input, 201)
    }

    /// Starts a research run on an open question; the run goes on without the caller.
    pub fn start_research_run(&self, key: &str) -> Result<RunDto, ClientError> {
        self.post_empty(&format!("/questions/{key}/research-runs"), 202)
    }

    /// Starts a plan run on a ticket; the run goes on without the caller.
    pub fn start_plan_run(&self, key: &str) -> Result<RunDto, ClientError> {
        self.post_empty(&format!("/tickets/{key}/plan-runs"), 202)
    }

    pub fn add_question_comment(
        &self,
        key: &str,
        input: &AddQuestionComment,
    ) -> Result<QuestionCommentDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/questions/{key}/comments")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    pub fn add_step(&self, key: &str, input: &AddStep) -> Result<TicketDetailDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/tickets/{key}/steps")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    pub fn change_step(
        &self,
        key: &str,
        step: Uuid,
        input: &ChangeStep,
    ) -> Result<TicketDetailDto, ClientError> {
        let mut response = self
            .agent
            .patch(&self.url(&format!("/tickets/{key}/steps/{step}")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Removes the step; `expect_text` must hold what the step said when it was read.
    pub fn remove_step(
        &self,
        key: &str,
        step: Uuid,
        version: Option<i64>,
        expect_text: Option<&str>,
        author: Option<&str>,
        session: Option<&str>,
    ) -> Result<TicketDetailDto, ClientError> {
        let mut request = self
            .agent
            .delete(&self.url(&format!("/tickets/{key}/steps/{step}")))
            .header("Authorization", &self.authorization()?);
        if let Some(version) = version {
            request = request.query("version", version.to_string());
        }
        if let Some(text) = expect_text {
            request = request.query("expectText", text);
        }
        if let Some(author) = author {
            request = request.query("author", author);
        }
        if let Some(session) = session {
            request = request.query("session", session);
        }
        let mut response = request.call().map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn move_step(
        &self,
        key: &str,
        step: Uuid,
        input: &MoveStep,
    ) -> Result<TicketDetailDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/tickets/{key}/steps/{step}/move")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Posts `input` to a ticket action such as `claim`, answered with the ticket.
    fn ticket_action<T: serde::Serialize>(
        &self,
        key: &str,
        action: &str,
        input: &T,
    ) -> Result<TicketDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/tickets/{key}/{action}")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Starts the ticket in a worktree; `key` must be a ticket key as for [`Client::get_ticket`].
    pub fn start_ticket(
        &self,
        key: &str,
        input: &StartTicket,
    ) -> Result<StartedTicket, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/tickets/{key}/start")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// Moves the ticket to done, then marks its worktree ready and cleans up; never merges.
    pub fn finish_ticket(
        &self,
        key: &str,
        input: &FinishTicket,
    ) -> Result<FinishedTicket, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/tickets/{key}/done")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn claim_ticket(&self, key: &str, input: &ClaimTicket) -> Result<TicketDto, ClientError> {
        self.ticket_action(key, "claim", input)
    }

    pub fn release_ticket(
        &self,
        key: &str,
        input: &ReleaseTicket,
    ) -> Result<TicketDto, ClientError> {
        self.ticket_action(key, "release", input)
    }

    pub fn accept_ticket(&self, key: &str, input: &AcceptTicket) -> Result<TicketDto, ClientError> {
        self.ticket_action(key, "accept", input)
    }

    /// Queues the merge of a done or merge-blocked ticket again and tries it.
    pub fn retry_merge(&self, key: &str, input: &RetryMerge) -> Result<MergeRetryDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/tickets/{key}/merge")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn return_ticket(&self, key: &str, input: &ReturnReview) -> Result<TicketDto, ClientError> {
        self.ticket_action(key, "review/return", input)
    }

    pub fn claim_planning(
        &self,
        key: &str,
        input: &ClaimPlanning,
    ) -> Result<TicketDto, ClientError> {
        self.ticket_action(key, "planning-claim", input)
    }

    pub fn release_planning(
        &self,
        key: &str,
        agent: &str,
        session: Option<&str>,
    ) -> Result<(), ClientError> {
        let mut request = self
            .agent
            .delete(&self.url(&format!("/tickets/{key}/planning-claim")))
            .header("Authorization", &self.authorization()?)
            .query("agent", agent);
        if let Some(session) = session {
            request = request.query("session", session);
        }
        let mut response = request.call().map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 204)
    }

    pub fn write_plan(&self, key: &str, input: &WritePlan) -> Result<TicketDto, ClientError> {
        let mut response = self
            .agent
            .put(&self.url(&format!("/tickets/{key}/plan")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    /// The next ticket that needs planning, of one board or project or of all.
    pub fn next_planning(
        &self,
        project: Option<Uuid>,
        board: Option<Uuid>,
    ) -> Result<NextPlanning, ClientError> {
        let mut request = self
            .agent
            .get(&self.url("/planning-queue"))
            .header("Authorization", &self.authorization()?);
        if let Some(project) = project {
            request = request.query("projectId", project.to_string());
        }
        if let Some(board) = board {
            request = request.query("boardId", board.to_string());
        }
        let mut response = request.call().map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn claim_next_planning(
        &self,
        input: &NextPlanningClaim,
    ) -> Result<NextPlanning, ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/planning-queue/claim"))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn delete_ticket(&self, key: &str, version: Option<i64>) -> Result<(), ClientError> {
        let route = match version {
            Some(version) => format!("/tickets/{key}?version={version}"),
            None => format!("/tickets/{key}"),
        };
        let mut response = self
            .agent
            .delete(&self.url(&route))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 204)
    }

    pub fn hook_events(&self, limit: u32) -> Result<HookEventList, ClientError> {
        self.get_json(&format!("/hook-events?limit={limit}"))
    }

    /// Notes of a board; `query` holds pairs such as `("state", "all")`.
    pub fn list_notes(
        &self,
        project: Uuid,
        board: Uuid,
        query: &[(&str, &str)],
    ) -> Result<NoteList, ClientError> {
        let mut request = self
            .agent
            .get(&self.url(&format!("/projects/{project}/boards/{board}/notes")))
            .header("Authorization", &self.authorization()?);
        for (name, value) in query {
            request = request.query(*name, *value);
        }
        let mut response = request.call().map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn add_note(
        &self,
        project: Uuid,
        board: Uuid,
        input: &CreateNote,
    ) -> Result<NoteDto, ClientError> {
        self.post_json(
            &format!("/projects/{project}/boards/{board}/notes"),
            input,
            201,
        )
    }

    /// `number` goes into the path as typed; the daemon checks it.
    pub fn get_note(&self, number: &str) -> Result<NoteDto, ClientError> {
        self.get_json(&format!("/notes/{}", segment(number)))
    }

    pub fn seen_note(&self, number: &str, input: &SeenNote) -> Result<NoteDto, ClientError> {
        self.post_json(&format!("/notes/{}/seen", segment(number)), input, 200)
    }

    pub fn promote_note(
        &self,
        number: &str,
        input: &PromoteNote,
    ) -> Result<PromotedNote, ClientError> {
        self.post_json(&format!("/notes/{}/promote", segment(number)), input, 200)
    }

    pub fn drop_note(&self, number: &str, input: &DropNote) -> Result<NoteDto, ClientError> {
        self.post_json(&format!("/notes/{}/drop", segment(number)), input, 200)
    }

    /// The inbox of one board or of all; `kind` such as `merge,accept` keeps only those.
    pub fn get_inbox(
        &self,
        board: Option<Uuid>,
        kind: Option<&str>,
    ) -> Result<InboxList, ClientError> {
        let mut request = self
            .agent
            .get(&self.url("/me/inbox"))
            .header("Authorization", &self.authorization()?);
        if let Some(board) = board {
            request = request.query("boardId", board.to_string());
        }
        if let Some(kind) = kind {
            request = request.query("kind", kind);
        }
        let mut response = request.call().map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    fn post_json<I: serde::Serialize, T: DeserializeOwned>(
        &self,
        route: &str,
        input: &I,
        expected: u16,
    ) -> Result<T, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(route))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, expected)
    }

    /// A POST to a route that takes no body; sending one makes the daemon reset the connection.
    fn post_empty<T: DeserializeOwned>(
        &self,
        route: &str,
        expected: u16,
    ) -> Result<T, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(route))
            .header("Authorization", &self.authorization()?)
            .send_empty()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, expected)
    }

    fn get_json<T: DeserializeOwned>(&self, route: &str) -> Result<T, ClientError> {
        let bearer = self.authorization()?;
        let mut response = self.get_retrying(&self.url(route), Some(&bearer))?;
        decode(&mut response, 200)
    }

    /// A GET changes nothing, so a refused or reset connection is tried again a few times.
    fn get_retrying(&self, url: &str, bearer: Option<&str>) -> Result<Response<Body>, ClientError> {
        self.get_within(url, bearer, None)
    }

    /// A GET repeated on connection failures, with its own overall limit when given.
    fn get_within(
        &self,
        url: &str,
        bearer: Option<&str>,
        limit: Option<Duration>,
    ) -> Result<Response<Body>, ClientError> {
        let mut attempt = 1;
        loop {
            let mut request = self.agent.get(url);
            if let Some(limit) = limit {
                request = request.config().timeout_global(Some(limit)).build();
            }
            if let Some(bearer) = bearer {
                request = request.header("Authorization", bearer);
            }
            match request.call() {
                Ok(response) => return Ok(response),
                Err(source) if attempt < GET_ATTEMPTS && is_transient(&source) => {
                    attempt += 1;
                    std::thread::sleep(GET_PAUSE);
                }
                Err(source) => return Err(self.unreachable(source)),
            }
        }
    }

    fn authorization(&self) -> Result<String, ClientError> {
        let token = self.token.as_ref().ok_or(ClientError::MissingToken)?;
        Ok(format!("Bearer {}", token.expose()))
    }

    fn url(&self, route: &str) -> String {
        format!("http://{}{API_PREFIX}{route}", self.address)
    }

    fn unreachable(&self, source: ureq::Error) -> ClientError {
        ClientError::Unreachable {
            address: self.address,
            source: Box::new(source),
        }
    }
}

/// A path segment that keeps a typed value from reaching another route.
fn customization_route(point: ExtensionPointDto, name: &str) -> String {
    let point = match point {
        ExtensionPointDto::Skill => "skill",
        ExtensionPointDto::OutputStyle => "output-style",
    };
    format!("/customizations/{point}/{}", segment(name))
}

fn segment(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn expect_empty(response: &mut Response<Body>, expected: u16) -> Result<(), ClientError> {
    if response.status().as_u16() == expected {
        return Ok(());
    }
    decode::<serde_json::Value>(response, expected).map(|_| ())
}

fn decode<T: DeserializeOwned>(
    response: &mut Response<Body>,
    expected: u16,
) -> Result<T, ClientError> {
    let status = response.status().as_u16();
    if status == expected {
        return response
            .body_mut()
            .read_json()
            .map_err(|error| ClientError::Protocol(error.to_string()));
    }
    match response.body_mut().read_json::<Problem>() {
        Ok(problem) => Err(refusal(status, problem)),
        Err(_) => Err(ClientError::Protocol(format!("unexpected status {status}"))),
    }
}

fn refusal(status: u16, problem: Problem) -> ClientError {
    let detail = describe(&problem);
    match problem.confirmation {
        Some(facts) if problem.code == "theme-confirmation-required" => {
            ClientError::ConfirmationRequired {
                detail,
                facts: Box::new(facts),
            }
        }
        _ => ClientError::Refused {
            status,
            detail,
            code: problem.code,
        },
    }
}

/// The detail, followed by the refused fields when there are some.
fn describe(problem: &Problem) -> String {
    if problem.errors.is_empty() {
        return problem.detail.clone();
    }
    // fields with the same message are listed once
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for error in &problem.errors {
        match groups
            .iter_mut()
            .find(|(message, _)| *message == error.message)
        {
            Some((_, fields)) => fields.push(&error.field),
            None => groups.push((&error.message, vec![&error.field])),
        }
    }
    let fields: Vec<String> = groups
        .iter()
        .map(|(message, fields)| format!("{}: {message}", fields.join(", ")))
        .collect();
    format!("{}: {}", problem.detail, fields.join("; "))
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::Arc;

    use nullradix_api::{AllowedHosts, ApiState};
    use nullradix_app::auth::AuthService;
    use nullradix_app::hooks::HookService;
    use nullradix_app::projects::ProjectService;
    use nullradix_app::runs::RunService;
    use nullradix_app::{ChangeSignal, SystemClock, UuidV7Generator};
    use nullradix_runner::{LocalRunner, SupervisorCommand};
    use nullradix_store::Store;

    use super::*;

    const FAST: Duration = Duration::from_millis(500);
    // the in-process daemon of a test may start slowly on a loaded runner
    const PATIENT: Duration = Duration::from_secs(10);

    struct TestDaemon {
        address: SocketAddr,
        token: Secret,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
    }

    impl Drop for TestDaemon {
        fn drop(&mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
        }
    }

    async fn local_runner(
        store: &Arc<Store>,
        signal: &ChangeSignal,
    ) -> (
        Arc<LocalRunner>,
        Arc<nullradix_app::repositories::RunnerService>,
    ) {
        let runner = Arc::new(LocalRunner::new(
            std::env::temp_dir().join("nullradix-client-tests"),
            SupervisorCommand {
                program: std::path::PathBuf::from("nullradix-missing-supervisor"),
                args: Vec::new(),
            },
            None,
        ));
        let runners = nullradix_app::repositories::RunnerService::start(
            store.clone(),
            store.clone(),
            runner.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        )
        .await
        .unwrap();
        (runner, Arc::new(runners))
    }

    /// Claude Code is not installed, so a login ends at once with the reason.
    fn login_service(
        accounts: &Arc<nullradix_app::accounts::AccountService>,
    ) -> Arc<nullradix_app::login::LoginService> {
        Arc::new(nullradix_app::login::LoginService::new(
            Arc::new(nullradix_runner::LocalLoginRunner::with_launcher(
                String::from("nullradix-no-such-claude"),
                Vec::new(),
            )),
            accounts.clone(),
            Arc::new(nullradix_runner::LocalClaudeLogins::new(
                Some(std::env::temp_dir().join("nullradix-client-tests-home")),
                None,
            )),
            Arc::new(SystemClock),
        ))
    }

    fn usage_service() -> Arc<nullradix_app::usage::UsageService> {
        Arc::new(nullradix_app::usage::UsageService::new(
            Arc::new(nullradix_runner::LocalClaudeLogins::new(
                Some(std::env::temp_dir().join("nullradix-client-tests-home")),
                None,
            )),
            Arc::new(nullradix_runner::CurlUsageApi::anthropic()),
            Arc::new(SystemClock),
        ))
    }

    fn account_service(
        store: &Arc<Store>,
        signal: &ChangeSignal,
    ) -> Arc<nullradix_app::accounts::AccountService> {
        Arc::new(nullradix_app::accounts::AccountService::new(
            store.clone(),
            Arc::new(nullradix_app::secrets::MemorySecretStore::new()),
            Arc::new(nullradix_runner::LocalAccountFolders::new(
                std::env::temp_dir().join("nullradix-client-tests-accounts"),
            )),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ))
    }

    fn dispatcher_service(
        store: &Arc<Store>,
        signal: &ChangeSignal,
    ) -> Arc<nullradix_app::dispatcher_config::DispatcherConfigService> {
        Arc::new(
            nullradix_app::dispatcher_config::DispatcherConfigService::new(
                store.clone(),
                store.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
                signal.clone(),
            ),
        )
    }

    fn project_service(store: &Arc<Store>, signal: &ChangeSignal) -> Arc<ProjectService> {
        Arc::new(ProjectService::new(
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ))
    }

    fn customization_service(
        store: &Arc<Store>,
        signal: &ChangeSignal,
    ) -> Arc<nullradix_app::customizations::CustomizationService> {
        Arc::new(nullradix_app::customizations::CustomizationService::new(
            store.clone(),
            Arc::new(nullradix_app::customizations::NoStandards),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ))
    }

    fn theme_service(
        store: &Arc<Store>,
        signal: &ChangeSignal,
    ) -> Arc<nullradix_app::themes::ThemeService> {
        Arc::new(nullradix_app::themes::ThemeService::new(
            store.clone(),
            Arc::new(nullradix_app::themes::UnavailableFiles),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ))
    }

    fn catalog_service(
        store: &Arc<Store>,
        themes: &Arc<nullradix_app::themes::ThemeService>,
        signal: &ChangeSignal,
    ) -> Arc<nullradix_app::theme_catalog::CatalogService> {
        Arc::new(nullradix_app::theme_catalog::CatalogService::new(
            store.clone(),
            Arc::new(nullradix_app::theme_catalog::NoNetwork),
            themes.clone(),
            Arc::new(SystemClock),
            signal.clone(),
            nullradix_domain::Reach::Public,
        ))
    }

    #[allow(clippy::too_many_lines, reason = "wires every service")]
    async fn test_state(ready: std::sync::mpsc::Sender<Secret>) -> ApiState {
        let store = Arc::new(Store::in_memory().await.unwrap());
        let auth = Arc::new(AuthService::new(
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
        ));
        ready.send(auth.local_token(None).await.unwrap()).unwrap();
        let signal = ChangeSignal::new();
        let projects = project_service(&store, &signal);
        let accounts = account_service(&store, &signal);
        let (runner, runners) = local_runner(&store, &signal).await;
        let runs = RunService::new(
            store.clone(),
            runner.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
            FAST,
        );
        let dispatcher_config = dispatcher_service(&store, &signal);
        let dispatch_runs = Arc::new(nullradix_app::dispatch::runs::DispatchRuns::new(
            runs.clone(),
            dispatcher_config.clone(),
            accounts.clone(),
            store.clone(),
            store.clone(),
            runner,
        ));
        let (sessions, favorites, hooks) = hook_services(&store, &runners, &signal);
        let boards = Arc::new(nullradix_app::boards::BoardService::new(
            store.clone(),
            store.clone(),
            store.clone(),
            runners.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let tickets = Arc::new(nullradix_app::tickets::TicketService::new(
            store.clone(),
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let comments = Arc::new(nullradix_app::comments::CommentService::new(
            store.clone(),
            store.clone(),
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let questions = Arc::new(nullradix_app::questions::QuestionService::new(
            store.clone(),
            store.clone(),
            store.clone(),
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let worktrees = Arc::new(nullradix_app::worktrees::WorktreeService::new(
            tickets.clone(),
            store.clone(),
            store.clone(),
            runners.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let reviews = Arc::new(nullradix_app::review::ReviewService::new(
            nullradix_app::review::ReviewParts {
                tickets: tickets.clone(),
                worktrees: store.clone(),
                runners: runners.clone(),
                reviews: store.clone(),
                clock: Arc::new(SystemClock),
                ids: Arc::new(UuidV7Generator),
                signal: signal.clone(),
            },
        ));
        let person = nullradix_app::boards::local_person(
            store.as_ref(),
            "tester",
            &UuidV7Generator,
            &SystemClock,
        )
        .await
        .unwrap();
        let notes = note_service(&store, &tickets, &signal);
        let dispatcher_config = dispatcher_service(&store, &signal);
        let prompts = Arc::new(
            nullradix_app::dispatcher_prompts::DispatcherPromptService::new(
                dispatcher_config.clone(),
                tickets.clone(),
                questions.clone(),
            ),
        );
        let usage = usage_service();
        let assembled = nullradix_app::dispatch::assemble::assemble(
            nullradix_app::dispatch::assemble::DispatcherDeps {
                config: dispatcher_config.clone(),
                states: store.clone(),
                log: store.clone(),
                accounts: accounts.clone(),
                limits: usage.clone(),
                pauses: store.clone(),
                samples: store.clone(),
                setup: Arc::new(nullradix_runner::LocalSetup::default()),
                detached: Arc::new(nullradix_runner::LocalDetached::new(None)),
                tracking: store.clone(),
                questions: questions.clone(),
                actors: store.clone(),
                boards: store.clone(),
                runners: runners.clone(),
                runs: runs.clone(),
                dispatch: dispatch_runs.clone(),
                prompts: prompts.clone(),
                tickets: tickets.clone(),
                comments: comments.clone(),
                worktrees: worktrees.clone(),
                worktree_repository: store.clone(),
                merge_jobs: store.clone(),
                merge_git: Arc::new(nullradix_runner::LocalMergeGit::new(None)),
                reviews: reviews.clone(),
                clock: Arc::new(SystemClock),
                ids: Arc::new(UuidV7Generator),
                signal: signal.clone(),
                local_time: Arc::new(nullradix_app::dispatch::local_time::UtcTime),
                notifier: Arc::new(nullradix_app::notifier::NoNotifier),
                interval: None,
            },
        );
        let themes = theme_service(&store, &signal);
        let catalog = catalog_service(&store, &themes, &signal);
        ApiState {
            auth,
            projects,
            accounts: accounts.clone(),
            dispatcher_prompts: prompts,
            dispatcher: assembled.service,
            dispatch_stats: Arc::new(nullradix_app::dispatch::stats::DispatchStatsService::new(
                store.clone(),
                dispatcher_config.clone(),
                store.clone(),
                Arc::new(SystemClock),
                Arc::new(nullradix_app::dispatch::local_time::UtcTime),
            )),
            account_pauses: assembled.pauses,
            dispatcher_config,
            usage,
            logins: login_service(&accounts),
            boards,
            tickets,
            comments,
            questions,
            worktrees,
            reviews,
            decisions: assembled.decisions,
            merges: assembled.merges,
            retries: assembled.retries,
            notes,
            inbox: inbox_service(&store, &signal),
            runners,
            person: Arc::new(person),
            runs,
            dispatch_runs,
            hooks,
            sessions,
            favorites,
            themes: themes.clone(),
            theme_assets: Arc::new(nullradix_app::theme_fetch::ThemeAssets::new(
                themes.clone(),
                Arc::new(nullradix_app::theme_fetch::NoAssetNetwork),
                Arc::new(nullradix_app::theme_fetch::NoZipEntries),
                Arc::new(nullradix_app::theme_fetch::NoAssetCache),
            )),
            theme_previews: Arc::new(nullradix_app::theme_preview::ThemePreviews::new(
                catalog.clone(),
                Arc::new(nullradix_app::theme_catalog::NoNetwork),
                Arc::new(nullradix_app::theme_preview::NoPreviewCache),
            )),
            catalog,
            claude_theme: Arc::new(nullradix_app::claude_theme::ClaudeThemeService::new(
                store.clone(),
                themes.clone(),
                store.clone(),
                Arc::new(nullradix_app::claude_theme::NoSlots),
                Arc::new(SystemClock),
                signal.clone(),
            )),
            autostart: Arc::new(nullradix_app::autostart::AutostartService::new(Arc::new(
                nullradix_app::autostart::NoAutostart,
            ))),
            customizations: customization_service(&store, &signal),
            snippets: Arc::new(nullradix_app::snippets::SnippetService::new(
                store.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
                signal.clone(),
            )),
            changes: store,
            signal,
            shutdown: tokio_util::sync::CancellationToken::new(),
            allowed_hosts: AllowedHosts::loopback_and([]),
            limits: nullradix_api::Limits::default(),
            secure_cookies: false,
            web_dir: None,
        }
    }

    fn hook_services(
        store: &Arc<Store>,
        runners: &Arc<nullradix_app::repositories::RunnerService>,
        signal: &ChangeSignal,
    ) -> (
        Arc<nullradix_app::sessions::SessionService>,
        Arc<nullradix_app::favorites::FavoriteService>,
        Arc<HookService>,
    ) {
        let favorites = Arc::new(nullradix_app::favorites::FavoriteService::new(
            store.clone(),
            runners.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let sessions = Arc::new(nullradix_app::sessions::SessionService::new(
            store.clone(),
            runners.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let hooks = Arc::new(
            HookService::new(store.clone(), Arc::new(SystemClock)).with_sessions(sessions.clone()),
        );
        (sessions, favorites, hooks)
    }

    fn note_service(
        store: &Arc<Store>,
        tickets: &Arc<nullradix_app::tickets::TicketService>,
        signal: &nullradix_app::ChangeSignal,
    ) -> Arc<nullradix_app::notes::NoteService> {
        Arc::new(nullradix_app::notes::NoteService::new(
            store.clone(),
            tickets.clone(),
            store.clone(),
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ))
    }

    fn inbox_service(
        store: &Arc<Store>,
        signal: &nullradix_app::ChangeSignal,
    ) -> Arc<nullradix_app::inbox::InboxService> {
        Arc::new(nullradix_app::inbox::InboxService::new(
            store.clone(),
            Arc::new(SystemClock),
            signal.clone(),
        ))
    }

    fn spawn_daemon() -> TestDaemon {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let (ready, token) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let state = test_state(ready).await;
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let shutdown = async {
                    let _ = stopped.await;
                };
                nullradix_api::serve(
                    listener,
                    nullradix_api::router(state),
                    &nullradix_api::Limits::default(),
                    shutdown,
                )
                .await
                .unwrap();
            });
        });
        TestDaemon {
            address,
            token: token.recv().unwrap(),
            stop: Some(stop),
        }
    }

    #[test]
    fn handshakes_with_a_running_daemon() {
        let daemon = spawn_daemon();
        let health = Client::new(daemon.address, PATIENT, PATIENT)
            .handshake()
            .unwrap();
        assert_eq!(health.api_version, API_VERSION);
        assert_eq!(health.status, "ok");
    }

    #[test]
    fn creates_handoffs_with_the_local_token_only() {
        let daemon = spawn_daemon();
        let client = Client::new(daemon.address, PATIENT, PATIENT);
        assert!(matches!(
            client.create_handoff(),
            Err(ClientError::MissingToken)
        ));

        let handoff = client
            .clone()
            .with_token(daemon.token.clone())
            .create_handoff()
            .unwrap();
        assert!(handoff.path.ends_with(&handoff.code));

        let stranger = Secret::parse(&"c".repeat(64)).unwrap();
        let refused = client.with_token(stranger).create_handoff().unwrap_err();
        assert!(matches!(refused, ClientError::Refused { status: 401, .. }));
    }

    #[test]
    fn fields_that_share_a_message_are_listed_once_with_it() {
        use nullradix_api::FieldProblemDto;
        let field = |name: &str, message: &str| FieldProblemDto {
            field: name.to_owned(),
            message: message.to_owned(),
        };
        let problem = Problem {
            problem_type: String::new(),
            title: String::new(),
            status: 403,
            detail: String::from("no"),
            code: String::from("x"),
            existing: None,
            similar: Vec::new(),
            errors: vec![field("a", "m"), field("b", "n"), field("c", "m")],
            confirmation: None,
        };
        assert_eq!(describe(&problem), "no: a, c: m; b: n");
    }

    #[test]
    fn creates_step_up_codes_with_the_local_token_only() {
        let daemon = spawn_daemon();
        let client = Client::new(daemon.address, PATIENT, PATIENT);
        assert!(matches!(
            client.create_step_up_code(),
            Err(ClientError::MissingToken)
        ));
        let code = client
            .clone()
            .with_token(daemon.token.clone())
            .create_step_up_code()
            .unwrap();
        assert_eq!(code.path, format!("/#confirm={}", code.code));
        let stranger = Secret::parse(&"c".repeat(64)).unwrap();
        let refused = client
            .with_token(stranger)
            .create_step_up_code()
            .unwrap_err();
        assert!(matches!(refused, ClientError::Refused { status: 401, .. }));
    }

    #[test]
    fn stops_the_daemon_with_the_local_token_only() {
        let daemon = spawn_daemon();
        let client = Client::new(daemon.address, PATIENT, PATIENT);
        assert!(matches!(
            client.stop_daemon(),
            Err(ClientError::MissingToken)
        ));
        let stranger = Secret::parse(&"c".repeat(64)).unwrap();
        let refused = client.clone().with_token(stranger).stop_daemon();
        assert!(matches!(
            refused,
            Err(ClientError::Refused { status: 401, .. })
        ));
        client
            .with_token(daemon.token.clone())
            .stop_daemon()
            .unwrap();
    }

    #[test]
    fn a_fresh_daemon_has_no_active_runs() {
        let daemon = spawn_daemon();
        let client = Client::new(daemon.address, PATIENT, PATIENT).with_token(daemon.token.clone());
        assert_eq!(client.active_run_count().unwrap(), 0);
    }

    #[test]
    fn creates_turn_off_codes_with_the_local_token_only() {
        let daemon = spawn_daemon();
        let client = Client::new(daemon.address, PATIENT, PATIENT);
        assert!(matches!(
            client.create_turn_off_code(),
            Err(ClientError::MissingToken)
        ));
        let code = client
            .clone()
            .with_token(daemon.token.clone())
            .create_turn_off_code()
            .unwrap();
        assert_eq!(code.code.len(), 64);
        let stranger = Secret::parse(&"c".repeat(64)).unwrap();
        let refused = client
            .with_token(stranger)
            .create_turn_off_code()
            .unwrap_err();
        assert!(matches!(refused, ClientError::Refused { status: 401, .. }));
    }

    #[test]
    fn manages_catalog_sources_and_reports_an_unreachable_one() {
        let daemon = spawn_daemon();
        let client = Client::new(daemon.address, PATIENT, PATIENT).with_token(daemon.token.clone());
        assert_eq!(client.catalog_sources().unwrap().version, 0);
        let refused = client
            .add_catalog_source("http://127.0.0.1/i.json")
            .unwrap_err();
        assert!(matches!(refused, ClientError::Refused { status: 422, .. }));

        let source = client
            .add_catalog_source("https://themes.example.com/index.json")
            .unwrap();
        let list = client.catalog_sources().unwrap();
        assert_eq!((list.items.len(), list.version), (1, 1));
        let catalog = client.browse_catalog().unwrap();
        assert_eq!(catalog.items.len(), 0);
        assert_eq!(
            catalog.sources[0].problem.as_ref().map(|p| p.code.as_str()),
            Some("catalog-unreachable")
        );
        let missing = client
            .install_from_catalog(&source.id, "night-blue", false)
            .unwrap_err();
        assert!(matches!(missing, ClientError::Refused { status: 503, .. }));
        client.remove_catalog_source(&source.id, 1).unwrap();
        assert_eq!(client.catalog_sources().unwrap().items.len(), 0);
    }

    const SHA: &str = "ab12";

    fn night_blue_dto() -> ThemeDto {
        ThemeDto {
            id: String::from("night-blue"),
            name: String::from("Night Blue"),
            package_version: String::from("1.0.0"),
            author: String::from("Ada"),
            sdk_version: String::from("^2.0"),
            color_scheme: nullradix_api::ColorSchemeDto::Dark,
            start_color: None,
            preview_url: None,
            style_url: String::new(),
            slots: Vec::new(),
            network: Vec::new(),
            settings: Vec::new(),
            claude_theme: false,
            has_code: false,
            code_enabled: false,
            code_url: None,
            installed_at: 0,
            updated_at: 0,
            version: 1,
        }
    }

    /// Answers a catalog install: 201 for the source `plain`, else the refusal with the facts.
    fn spawn_catalog_daemon() -> (SocketAddr, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, requests) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut head = Vec::new();
                let mut byte = [0_u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let target = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                let (status, body) = if target.contains("/broken/") {
                    ("409 Conflict", String::from("not json"))
                } else if target.contains("/plain/") {
                    (
                        "201 Created",
                        serde_json::to_string(&night_blue_dto()).unwrap(),
                    )
                } else {
                    let facts = serde_json::json!({
                        "hasCode": true, "domains": [], "claudeTheme": false, "update": false,
                        "added": { "code": true, "domains": [], "claudeTheme": false },
                        "id": "night-blue", "name": "Night Blue", "version": "1.0.0",
                        "sha256": SHA,
                    });
                    let problem = serde_json::json!({
                        "type": "about:blank", "title": "Conflict", "status": 409,
                        "detail": "night-blue contains code", "code": "theme-confirmation-required",
                        "confirmation": facts,
                    });
                    ("409 Conflict", problem.to_string())
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
                let _ = seen.send(target);
            }
        });
        (address, requests)
    }

    #[test]
    fn a_catalog_install_is_one_request_that_never_confirms() {
        let (address, requests) = spawn_catalog_daemon();
        let client = Client::new(address, PATIENT, PATIENT)
            .with_token(Secret::parse(&"c".repeat(64)).unwrap());
        let refusal = |replace: bool| match client
            .install_from_catalog("src", "night-blue", replace)
            .unwrap_err()
        {
            ClientError::ConfirmationRequired { facts, .. } => facts,
            other => panic!("{other:?}"),
        };
        let facts = refusal(false);
        assert_eq!(
            (facts.sha256.as_deref(), facts.name.as_str()),
            (Some(SHA), "Night Blue")
        );
        assert!(requests.recv().unwrap().ends_with("/install"));
        refusal(true);
        assert!(requests.recv().unwrap().ends_with("/install?replace=true"));

        let theme = client
            .install_from_catalog("plain", "night-blue", false)
            .unwrap();
        assert_eq!(theme.id, "night-blue");
        assert!(requests.recv().unwrap().ends_with("/install"));
        assert!(requests.try_recv().is_err());

        let broken = client
            .install_from_catalog("broken", "night-blue", false)
            .unwrap_err();
        assert!(matches!(broken, ClientError::Protocol(_)), "{broken:?}");
    }

    /// Sends the head of every request it gets, and answers with an empty theme list.
    fn spawn_echo_daemon() -> (SocketAddr, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, heads) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut head = Vec::new();
                let mut byte = [0_u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    head.push(byte[0]);
                }
                let body = r#"{"items":[],"active":"nullradix"}"#;
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
                let _ = seen.send(String::from_utf8_lossy(&head).to_lowercase());
            }
        });
        (address, heads)
    }

    #[test]
    fn requests_name_the_run_and_session_of_an_agent() {
        let (address, heads) = spawn_echo_daemon();
        let token = Secret::parse(&"c".repeat(64)).unwrap();
        let plain = Client::new(address, PATIENT, PATIENT).with_token(token);
        plain.list_themes().unwrap();
        let head = heads.recv().unwrap();
        assert!(!head.contains("x-nullradix-"), "{head}");

        let agent = plain.with_context(AgentContext {
            run: Some(String::from("run-1")),
            session: Some(String::from("session 1")),
        });
        agent.list_themes().unwrap();
        let head = heads.recv().unwrap();
        assert!(head.contains("x-nullradix-run: run-1\r\n"), "{head}");
        assert!(
            head.contains("x-nullradix-session: session 1\r\n"),
            "{head}"
        );

        // a value that cannot be a header still marks the request
        let odd = agent.with_timeout(PATIENT).with_context(AgentContext {
            run: Some(String::from("bad\nvalue")),
            session: None,
        });
        odd.list_themes().unwrap();
        let head = heads.recv().unwrap();
        assert!(head.contains("x-nullradix-run: agent\r\n"), "{head}");
        assert!(!head.contains("x-nullradix-session"), "{head}");

        // a client with another limit keeps naming the agent
        let limited = agent.with_timeout(PATIENT);
        limited.list_themes().unwrap();
        assert!(heads.recv().unwrap().contains("x-nullradix-run: run-1"));
    }

    #[test]
    fn manages_accounts_and_run_tokens() {
        let daemon = spawn_daemon();
        let client = Client::new(daemon.address, PATIENT, PATIENT).with_token(daemon.token.clone());
        let list = client.list_accounts().unwrap();
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].slug, "default");

        let created = client
            .create_account(&CreateAccount {
                name: String::from("Max"),
                config_dir: None,
            })
            .unwrap();
        assert_eq!(created.slug, "max");
        let updated = client
            .update_account(
                created.id,
                &UpdateAccount {
                    version: created.version,
                    enabled: Some(false),
                    limit_week: Some(Some(80)),
                    ..UpdateAccount::default()
                },
            )
            .unwrap();
        assert!(!updated.enabled);
        assert_eq!(updated.limit_week, Some(80));
        let with_token = client
            .set_run_token(
                created.id,
                &SetRunToken {
                    token: String::from("sk-ant-oat01-fake-token"),
                    version: updated.version,
                },
            )
            .unwrap();
        assert!(with_token.has_run_token);
        let without = client
            .clear_run_token(created.id, with_token.version)
            .unwrap();
        assert!(!without.has_run_token);
        let resumed = client.resume_account(created.id).unwrap();
        assert_eq!(resumed.paused_until, None);
        client
            .delete_account(created.id, without.version, true)
            .unwrap();
        assert_eq!(client.list_accounts().unwrap().items.len(), 1);
        let gone = client.delete_account(created.id, 1, false).unwrap_err();
        assert!(matches!(gone, ClientError::Refused { status: 404, .. }));
        let gone = client.resume_account(created.id).unwrap_err();
        assert!(matches!(gone, ClientError::Refused { status: 404, .. }));
    }

    #[test]
    fn reads_the_login_and_limits_of_the_accounts() {
        let daemon = spawn_daemon();
        let client = Client::new(daemon.address, PATIENT, PATIENT).with_token(daemon.token.clone());
        for fresh in [false, true] {
            let status = client.account_usage(fresh).unwrap();
            assert_eq!(status.items.len(), 1);
            let default = &status.items[0];
            assert_eq!(default.slug, "default");
            assert!(!default.logged_in);
            assert!(!default.usage.available);
            assert_eq!(
                default.usage.reason,
                Some(nullradix_api::UsageReasonDto::NoLogin)
            );
        }
    }

    #[test]
    fn logs_accounts_in_and_reports_a_claude_that_cannot_start() {
        let daemon = spawn_daemon();
        let client = Client::new(daemon.address, PATIENT, PATIENT).with_token(daemon.token.clone());
        let started = client
            .create_account_login(&CreateAccountLogin {
                name: Some(String::from("Max")),
                email: Some(String::from("max@example.com")),
            })
            .unwrap();
        assert_eq!(started.account.slug, "max");
        assert_eq!(started.login.state, nullradix_api::LoginStateDto::Failed);
        assert!(started.login.output.contains("could not be started"));
        let shown = client.get_login(started.account.id).unwrap();
        assert_eq!(shown, started.login);
        let again = client
            .start_login(started.account.id, &StartLogin::default())
            .unwrap();
        assert_eq!(again.state, nullradix_api::LoginStateDto::Failed);
        let refused = client
            .send_login_code(
                started.account.id,
                &LoginCode {
                    code: String::from("abc"),
                },
            )
            .unwrap_err();
        assert!(matches!(refused, ClientError::Refused { status: 409, .. }));
        client.cancel_login(started.account.id).unwrap();
        let ghost = client.get_login(uuid::Uuid::from_u128(9)).unwrap_err();
        assert!(matches!(ghost, ClientError::Refused { status: 404, .. }));
    }
    #[test]
    fn typed_note_numbers_stay_one_path_segment() {
        assert_eq!(segment("12"), "12");
        assert_eq!(segment("1/drop"), "1%2Fdrop");
        assert_eq!(segment("a b?"), "a%20b%3F");
    }

    #[test]
    fn reports_an_unreachable_daemon() {
        let address = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let error = Client::new(address, FAST, FAST).health().unwrap_err();
        assert!(matches!(error, ClientError::Unreachable { .. }));
    }

    #[test]
    fn another_timeout_keeps_the_address_and_the_token() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let slow =
            Client::new(address, FAST, PATIENT).with_token(Secret::parse(&"c".repeat(64)).unwrap());
        let quick = slow.with_timeout(Duration::from_millis(200));
        assert_eq!(quick.address(), address);
        assert!(quick.token.is_some());
        // nobody answers: the new limit ends the call long before the old one
        let started = std::time::Instant::now();
        assert!(quick.health().is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
