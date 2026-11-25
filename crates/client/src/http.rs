use std::net::SocketAddr;
use std::time::Duration;

use nullradix_api::{
    API_PREFIX, API_VERSION, AcceptTicket, AddComment, AddQuestionComment, AddStep, AnswerQuestion,
    BoardDeleted, BoardDto, BoardList, ChangeStep, ClaimPlanning, ClaimSession, ClaimTicket,
    CommentDto, CommentList, CreateBoard, CreateNote, CreateProject, CreateQuestion, CreateTicket,
    DecideQuestion, DropNote, FavoriteDto, FavoriteList, FileDiffDto, FinishTicket, FinishedTicket,
    Handoff, Health, HookEventDto, HookEventList, HookKindDto, HookReplyDto, InboxList,
    MarkFavorite, MoveStep, MoveTicket, NextPlanning, NextPlanningClaim, NoteDto, NoteList,
    Problem, ProjectDto, ProjectList, PromoteNote, PromotedNote, QuestionCommentDto,
    QuestionDetailDto, QuestionList, ReleaseTicket, RenameBoard, RenameProject, RepositoryList,
    RunDto, RunList, RunOutput, RunnerList, SeenNote, SessionChangesDto, SessionDetailDto,
    SessionDto, SessionList, SetBoardRepository, StartRun, StartTicket, StartedTicket,
    TicketDetailDto, TicketDto, TicketList, TranscriptPageDto, UpdateTicket, WritePlan,
};
use nullradix_app::auth::Secret;
use serde::de::DeserializeOwned;
use ureq::http::Response;
use ureq::{Agent, Body};
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
    #[error("unexpected response from the daemon: {0}")]
    Protocol(String),
}

/// Blocking HTTP client for short-lived callers such as the CLI and hooks.
#[derive(Debug, Clone)]
pub struct Client {
    agent: Agent,
    address: SocketAddr,
    token: Option<Secret>,
}

impl Client {
    #[must_use]
    pub fn new(address: SocketAddr, connect_timeout: Duration, timeout: Duration) -> Self {
        let agent = Agent::config_builder()
            .timeout_connect(Some(connect_timeout))
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .proxy(None)
            .build()
            .into();
        Self {
            agent,
            address,
            token: None,
        }
    }

    #[must_use]
    pub fn with_token(mut self, token: Secret) -> Self {
        self.token = Some(token);
        self
    }

    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn health(&self) -> Result<Health, ClientError> {
        let mut response = self
            .agent
            .get(&self.url("/health"))
            .call()
            .map_err(|source| self.unreachable(source))?;
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

    pub fn claim_planning(
        &self,
        key: &str,
        input: &ClaimPlanning,
    ) -> Result<TicketDto, ClientError> {
        self.ticket_action(key, "planning-claim", input)
    }

    pub fn release_planning(&self, key: &str, agent: &str) -> Result<(), ClientError> {
        let mut response = self
            .agent
            .delete(&self.url(&format!("/tickets/{key}/planning-claim")))
            .header("Authorization", &self.authorization()?)
            .query("agent", agent)
            .call()
            .map_err(|source| self.unreachable(source))?;
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

    fn get_json<T: DeserializeOwned>(&self, route: &str) -> Result<T, ClientError> {
        let mut response = self
            .agent
            .get(&self.url(route))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
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
        Ok(problem) => Err(ClientError::Refused {
            status,
            code: problem.code,
            detail: problem.detail,
        }),
        Err(_) => Err(ClientError::Protocol(format!("unexpected status {status}"))),
    }
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

    async fn test_state(ready: std::sync::mpsc::Sender<Secret>) -> ApiState {
        let store = Arc::new(Store::in_memory().await.unwrap());
        let auth = Arc::new(AuthService::new(
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
        ));
        ready.send(auth.local_token(None).await.unwrap()).unwrap();
        let signal = ChangeSignal::new();
        let projects = Arc::new(ProjectService::new(
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let (runner, runners) = local_runner(&store, &signal).await;
        let runs = RunService::new(
            store.clone(),
            runner,
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
            FAST,
        );
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
        let person = nullradix_app::boards::local_person(
            store.as_ref(),
            "tester",
            &UuidV7Generator,
            &SystemClock,
        )
        .await
        .unwrap();
        let notes = note_service(&store, &tickets, &signal);
        ApiState {
            auth,
            projects,
            boards,
            tickets,
            comments,
            questions,
            worktrees,
            notes,
            inbox: inbox_service(&store, &signal),
            runners,
            person: Arc::new(person),
            runs,
            hooks,
            sessions,
            favorites,
            changes: store,
            signal,
            shutdown: tokio_util::sync::CancellationToken::new(),
            allowed_hosts: AllowedHosts::loopback_and([]),
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
                nullradix_api::serve(listener, nullradix_api::router(state), shutdown)
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
        let health = Client::new(daemon.address, FAST, FAST).handshake().unwrap();
        assert_eq!(health.api_version, API_VERSION);
        assert_eq!(health.status, "ok");
    }

    #[test]
    fn creates_handoffs_with_the_local_token_only() {
        let daemon = spawn_daemon();
        let client = Client::new(daemon.address, FAST, FAST);
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
}
