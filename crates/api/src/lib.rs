//! HTTP API served by the daemon.

mod accounts;
mod auth;
mod boards;
mod body_guard;
mod claude_theme;
mod comments;
mod customizations;
mod dispatch_runs;
mod dispatch_stats;
mod dispatcher_config;
mod dispatcher_control;
mod dispatcher_prompts;
mod events;
mod extract;
mod favorites;
mod health;
mod hooks;
mod host;
mod inbox;
mod login;
mod merge;
mod notes;
mod problem;
mod projects;
mod questions;
mod repositories;
mod review;
mod runs;
mod session_detail;
mod sessions;
mod theme_catalog;
mod themes;
mod tickets;
mod usage;
mod web;
mod worktrees;

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use nullradix_app::accounts::AccountService;
use nullradix_app::auth::AuthService;
use nullradix_app::boards::BoardService;
use nullradix_app::claude_theme::ClaudeThemeService;
use nullradix_app::comments::CommentService;
use nullradix_app::customizations::CustomizationService;
use nullradix_app::dispatch::accounts::AccountPauses;
use nullradix_app::dispatch::control::DispatcherService;
use nullradix_app::dispatch::review_decide::ReviewDecisions;
use nullradix_app::dispatch::runs::DispatchRuns;
use nullradix_app::dispatch::stats::DispatchStatsService;
use nullradix_app::dispatcher_config::DispatcherConfigService;
use nullradix_app::dispatcher_prompts::DispatcherPromptService;
use nullradix_app::favorites::FavoriteService;
use nullradix_app::hooks::HookService;
use nullradix_app::inbox::InboxService;
use nullradix_app::login::LoginService;
use nullradix_app::notes::NoteService;
use nullradix_app::projects::ProjectService;
use nullradix_app::questions::QuestionService;
use nullradix_app::repositories::RunnerService;
use nullradix_app::review::ReviewService;
use nullradix_app::runs::RunService;
use nullradix_app::sessions::SessionService;
use nullradix_app::theme_catalog::CatalogService;
use nullradix_app::themes::ThemeService;
use nullradix_app::tickets::TicketService;
use nullradix_app::usage::UsageService;
use nullradix_app::worktrees::WorktreeService;
use nullradix_app::{AppError, ChangeFeed, ChangeSignal};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceBuilder;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub use accounts::{AccountDto, AccountList, CreateAccount, SetRunToken, UpdateAccount};
pub use auth::{Handoff, Me, PrincipalKind, SESSION_COOKIE};
pub use boards::{BoardDeleted, BoardDto, BoardList, CreateBoard, RenameBoard, SetBoardRepository};
pub use claude_theme::{ClaudeThemeDto, ClaudeThemeStateDto, ClaudeThemeTargetDto, SetClaudeTheme};
pub use comments::{ActorKindDto, AddComment, CommentDto, CommentList, SystemMarkerDto};
pub use customizations::{
    BaseStateDto, CreateCustomization, CustomizationDto, CustomizationList, EffectiveDto,
    ExtensionPointDto, FindingDto as CustomizationFindingDto, OverlayKindDto, OverlayStateDto,
    RebaseCustomization, SourceDto, UpdateCustomization,
};
pub use dispatch_runs::{
    AbortCategoryDto, AbortDto, DeniedToolDto, DispatchDto, LineKindDto, ModelUsageDto,
    RunDetailDto, RunKindDto, RunProfileDto, RunResultDto, RunTriggerDto, RunUsageDto,
    ScreenshotDto, ScreenshotList, StartDispatchRunBody, TranscriptDto, TranscriptLineDto,
};
pub use dispatch_stats::{
    AbortReasonDto, DispatchStatsDto, OutcomeCountDto, ProfileRowDto, SeriesPointDto,
    SeriesUnitDto, StatsFilterDto, StatsKpisDto, StatsLadderDto, StatsSeriesDto, StatsUsageDto,
};
pub use dispatcher_config::{
    AccountStrategyDto, AutopilotSettingsDto, ClaudeSettingsDto, DispatcherConfigDto,
    DispatcherConfigView, JoinWorktreesDto, MergeStrategyDto, PlanningSettingsDto, ProfileDto,
    PromptTemplatesDto, ResearchSettingsDto, RuleConditionDto, RuleDto, TimeWindowDto,
    UpdateDispatcherConfig, WorktreeSetupStepDto,
};
pub use dispatcher_control::{
    DispatcherPhaseDto, DispatcherRunDto, DispatcherStatusDto, PassReportDto, PauseDispatcher,
    TicketReadinessDto, UpdateDispatcher,
};
pub use dispatcher_prompts::{
    PlaceholderDto, PreviewPrompt, PromptDetail, PromptDto, PromptKindDto, PromptListDto,
    PromptPreviewDto, SetPrompt,
};
pub use events::{ChangeDto, ChangeOpDto, EntityDto, StreamPosition};
pub use extract::{ApiJson, ApiPath, ApiQuery};
pub use favorites::{FavoriteDto, FavoriteList, MarkFavorite, ResumptionDto};
pub use health::Health;
pub use hooks::{HookEventDto, HookEventList, HookKindDto, HookReplyDto, RecordedHookDto};
pub use host::AllowedHosts;
pub use inbox::{
    InboxCounts, InboxItemDto, InboxItemTypeDto, InboxKindDto, InboxList, InboxOrder, InboxRefDto,
};
pub use login::{
    AccountLoginDto, CreateAccountLogin, LoginCode, LoginDto, LoginStateDto, StartLogin,
};
pub use merge::{
    MergeJobDto, MergeJobList, MergeOutcomeDto, MergePhaseDto, MergeRetryDto, RetryMerge,
};
pub use notes::{
    CreateNote, DropNote, NoteDto, NoteList, NoteStateDto, PromoteNote, PromotedNote, SeenNote,
};
pub use problem::{ApiError, FieldProblemDto, Problem, SimilarNoteDto};
pub use projects::{CreateProject, ProjectDto, ProjectList, RenameProject};
pub use questions::{
    AddQuestionComment, AnswerQuestion, CreateQuestion, DecideQuestion, DecisionDto, NewOption,
    OpenQuestionDto, OptionDto, QuestionCommentDto, QuestionCommentList, QuestionCounts,
    QuestionDetailDto, QuestionDto, QuestionList, QuestionStateDto, QuestionTicketDto, ResearchDto,
    SubmitResearch, WaitingDto,
};
pub use repositories::{
    CheckoutDto, RepositoryDto, RepositoryList, RunnerDto, RunnerList, RunnerOsDto,
};
pub use review::{
    AcceptReview, ReturnReview, ReviewDto, ReviewFileContentDto, ReviewFileDto, ReviewPatchDto,
    ReviewStateDto, TestRunDto,
};
pub use runs::{
    OutputLineDto, OutputStreamDto, RunDto, RunList, RunOutput, RunStatusDto, StartRun,
};
pub use session_detail::{
    ChangeOriginDto, ChangedFileDto, ClaimSession, ContextUsageDto, DiffHunkDto, DiffLineDto,
    DiffLineKindDto, FileDiffDto, FileStatusDto, SessionChangesDto, SessionDetailDto,
    SystemLevelDto, TodoItemDto, TodoStatusDto, TokenTotalsDto, ToolResultDto, TranscriptEntryDto,
    TranscriptInfoDto, TranscriptPageDto,
};
pub use sessions::{
    DisplayStateDto, SessionClientDto, SessionDto, SessionList, SessionStateDto, SessionTicketDto,
    SubagentDto, SubagentStateDto,
};
pub use theme_catalog::{
    AddCatalogSource, CatalogDto, CatalogProblemDto, CatalogSourceDto, CatalogSourceListDto,
    CatalogSourceStatusDto, CatalogStateDto, CatalogThemeDto, SkippedEntryDto,
};
pub use themes::{
    ChooseTheme, ColorSchemeDto, SaveThemeSettings, ThemeDto, ThemeListDto, ThemeOptionDto,
    ThemeSettingDto, ThemeSettingValueDto, ThemeSettingsDto,
};
pub use tickets::{
    AcceptTicket, AddStep, ChangeStep, ClaimPlanning, ClaimTicket, ColumnDto, CreateTicket,
    HistoryDto, ImportTickets, ImportedTickets, MoveStep, MoveTicket, NextPlanning,
    NextPlanningClaim, PriorityDto, ReleaseTicket, StepDto, TicketDetailDto, TicketDto, TicketList,
    TicketStatusDto, UpdateTicket, WritePlan,
};
pub use usage::{AccountStatusDto, AccountStatusList, UsageDto, UsageReasonDto, UsageWindowDto};
pub use worktrees::{
    CleanedWorktree, FinishTicket, FinishedTicket, ResumeOutcome, ResumedTicket, StartTicket,
    StartedTicket, TargetCreatedDto,
};

/// Major version of the HTTP contract; clients refuse to talk to a different one.
pub const API_VERSION: u32 = 1;

/// Prefix of every API route; the rest of the URL space belongs to the web UI.
pub const API_PREFIX: &str = "/api/v1";

#[derive(OpenApi)]
#[openapi(
    info(title = "NULLRADIX API"),
    components(schemas(Problem, ChangeDto, StreamPosition)),
    modifiers(&SecuritySchemes),
    security(("bearer" = []), ("session" = []))
)]
struct ApiDoc;

struct SecuritySchemes;

impl Modify for SecuritySchemes {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(SESSION_COOKIE))),
        );
    }
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState").finish_non_exhaustive()
    }
}

/// Everything the handlers need, built by the composition root.
#[derive(Clone)]
pub struct ApiState {
    pub auth: Arc<AuthService>,
    pub projects: Arc<ProjectService>,
    pub accounts: Arc<AccountService>,
    pub dispatcher_config: Arc<DispatcherConfigService>,
    pub dispatcher_prompts: Arc<DispatcherPromptService>,
    pub dispatcher: Arc<DispatcherService>,
    pub dispatch_stats: Arc<DispatchStatsService>,
    pub account_pauses: Arc<AccountPauses>,
    pub usage: Arc<UsageService>,
    pub logins: Arc<LoginService>,
    pub boards: Arc<BoardService>,
    pub tickets: Arc<TicketService>,
    pub comments: Arc<CommentService>,
    pub questions: Arc<QuestionService>,
    pub worktrees: Arc<WorktreeService>,
    pub reviews: Arc<ReviewService>,
    /// Accept and return of a held review.
    pub decisions: Arc<ReviewDecisions>,
    pub merges: Arc<nullradix_app::dispatch::merge_jobs::MergeQueue>,
    pub retries: Arc<nullradix_app::dispatch::merge_retry::MergeRetry>,
    pub notes: Arc<NoteService>,
    pub inbox: Arc<InboxService>,
    pub runners: Arc<RunnerService>,
    /// The person every token and browser session acts for.
    pub person: Arc<nullradix_domain::Actor>,
    pub runs: Arc<RunService>,
    pub dispatch_runs: Arc<DispatchRuns>,
    pub hooks: Arc<HookService>,
    pub sessions: Arc<SessionService>,
    pub favorites: Arc<FavoriteService>,
    pub themes: Arc<ThemeService>,
    pub catalog: Arc<CatalogService>,
    pub claude_theme: Arc<ClaudeThemeService>,
    pub customizations: Arc<CustomizationService>,
    pub changes: Arc<dyn ChangeFeed>,
    pub signal: ChangeSignal,
    /// Cancelled when the daemon stops; long-lived responses end with it.
    pub shutdown: CancellationToken,
    pub allowed_hosts: AllowedHosts,
    pub secure_cookies: bool,
    /// Built web UI to serve next to the API, if any.
    pub web_dir: Option<PathBuf>,
}

fn account_routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(accounts::list_accounts, accounts::create_account))
        .routes(routes!(usage::account_usage))
        .routes(routes!(login::create_account_login))
        .routes(routes!(
            login::get_login,
            login::start_login,
            login::cancel_login
        ))
        .routes(routes!(login::send_login_code))
        .routes(routes!(accounts::update_account, accounts::delete_account))
        .routes(routes!(accounts::set_run_token, accounts::clear_run_token))
        .routes(routes!(accounts::resume_account))
}

fn extension_routes(router: OpenApiRouter<ApiState>) -> OpenApiRouter<ApiState> {
    router
        .routes(routes!(themes::list_themes, themes::install_theme))
        .routes(routes!(themes::remove_theme))
        .routes(routes!(
            themes::theme_settings,
            themes::save_theme_settings,
            themes::reset_theme_settings
        ))
        .routes(routes!(themes::theme_file))
        .routes(routes!(themes::choose_theme))
        .routes(routes!(
            claude_theme::claude_theme,
            claude_theme::set_claude_theme
        ))
        .routes(routes!(
            theme_catalog::list_catalog_sources,
            theme_catalog::add_catalog_source
        ))
        .routes(routes!(theme_catalog::remove_catalog_source))
        .routes(routes!(theme_catalog::browse_catalog))
        .routes(routes!(theme_catalog::install_catalog_theme))
        .routes(routes!(
            customizations::list_customizations,
            customizations::create_customization
        ))
        .routes(routes!(
            customizations::get_customization,
            customizations::update_customization,
            customizations::delete_customization
        ))
        .routes(routes!(customizations::rebase_customization))
        .routes(routes!(customizations::effective_customization))
}

fn run_routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(runs::list_runs, runs::start_run))
        .routes(routes!(runs::get_run))
        .routes(routes!(runs::cancel_run))
        .routes(routes!(runs::run_output))
        .routes(routes!(dispatch_runs::run_transcript))
        .routes(routes!(dispatch_runs::list_screenshots))
        .routes(routes!(dispatch_runs::screenshot))
}

fn review_routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(merge::list_merges))
        .routes(routes!(merge::retry_merge))
        .routes(routes!(review::get_review))
        .routes(routes!(review::get_review_diff))
        .routes(routes!(review::get_review_file))
        .routes(routes!(review::accept_review))
        .routes(routes!(review::return_review))
}

fn dispatcher_routes() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(
            dispatcher_config::get_dispatcher_config,
            dispatcher_config::update_dispatcher_config
        ))
        .routes(routes!(dispatcher_prompts::list_prompts))
        .routes(routes!(
            dispatcher_prompts::get_prompt,
            dispatcher_prompts::set_prompt,
            dispatcher_prompts::reset_prompt
        ))
        .routes(routes!(dispatcher_prompts::preview_prompt))
        .routes(routes!(dispatch_stats::get_dispatch_stats))
        .routes(routes!(dispatch_runs::start_dispatch_run))
        .routes(routes!(
            dispatcher_control::get_dispatcher,
            dispatcher_control::update_dispatcher
        ))
        .routes(routes!(dispatcher_control::pause_dispatcher))
        .routes(routes!(dispatcher_control::resume_dispatcher))
        .routes(routes!(dispatcher_control::run_pass))
        .routes(routes!(questions::submit_research))
        .routes(routes!(dispatch_runs::start_research_run))
        .routes(routes!(dispatch_runs::start_plan_run))
}

fn routes() -> OpenApiRouter<ApiState> {
    let v1 = OpenApiRouter::new()
        .routes(routes!(health::health))
        .routes(routes!(auth::create_handoff))
        .routes(routes!(auth::redeem_handoff))
        .routes(routes!(auth::logout))
        .routes(routes!(auth::me))
        .routes(routes!(events::events))
        .routes(routes!(projects::list_projects, projects::create_project))
        .routes(routes!(
            projects::get_project,
            projects::rename_project,
            projects::delete_project
        ))
        .routes(routes!(boards::list_all_boards))
        .routes(routes!(boards::list_boards, boards::create_board))
        .routes(routes!(
            boards::get_board,
            boards::rename_board,
            boards::delete_board
        ))
        .routes(routes!(
            boards::set_board_repository,
            boards::clear_board_repository
        ))
        .routes(routes!(repositories::list_runners))
        .routes(routes!(repositories::list_repositories))
        .routes(routes!(tickets::list_tickets))
        .routes(routes!(tickets::create_ticket))
        .routes(routes!(tickets::import_tickets))
        .routes(routes!(
            tickets::get_ticket,
            tickets::update_ticket,
            tickets::delete_ticket
        ))
        .routes(routes!(tickets::move_ticket))
        .routes(routes!(tickets::add_step))
        .routes(routes!(comments::list_comments, comments::add_comment))
        .routes(routes!(tickets::change_step, tickets::remove_step))
        .routes(routes!(tickets::move_step))
        .routes(routes!(
            questions::list_questions,
            questions::create_question
        ))
        .routes(routes!(questions::get_question))
        .routes(routes!(questions::answer_question))
        .routes(routes!(questions::discard_question))
        .routes(routes!(questions::reopen_question))
        .routes(routes!(
            questions::list_question_comments,
            questions::add_question_comment
        ))
        .routes(routes!(tickets::claim_ticket))
        .routes(routes!(worktrees::start_ticket))
        .routes(routes!(worktrees::finish_ticket))
        .routes(routes!(worktrees::resume_ticket))
        .routes(routes!(tickets::release_ticket))
        .routes(routes!(tickets::accept_ticket))
        .routes(routes!(tickets::write_plan))
        .routes(routes!(tickets::claim_planning, tickets::release_planning))
        .routes(routes!(tickets::next_planning))
        .routes(routes!(tickets::claim_next_planning))
        .routes(routes!(notes::list_notes, notes::add_note))
        .routes(routes!(notes::get_note))
        .routes(routes!(notes::seen_note))
        .routes(routes!(notes::promote_note))
        .routes(routes!(notes::drop_note))
        .routes(routes!(inbox::get_inbox))
        .routes(routes!(inbox::save_inbox_order, inbox::clear_inbox_order))
        .routes(routes!(sessions::list_sessions))
        .routes(routes!(sessions::get_session))
        .routes(routes!(session_detail::session_detail))
        .routes(routes!(session_detail::session_transcript))
        .routes(routes!(session_detail::session_changes))
        .routes(routes!(session_detail::session_file_diff))
        .routes(routes!(session_detail::claim_session))
        .routes(routes!(favorites::list_favorites))
        .routes(routes!(
            favorites::mark_favorite,
            favorites::unmark_favorite
        ))
        .routes(routes!(sessions::list_ticket_sessions))
        .routes(routes!(hooks::receive_hook))
        .routes(routes!(hooks::list_hook_events))
        .merge(account_routes())
        .merge(run_routes())
        .merge(review_routes());
    let v1 = extension_routes(v1).merge(dispatcher_routes());
    let mut doc = ApiDoc::openapi();
    doc.info.version = API_VERSION.to_string();
    doc.info.license = Some(
        utoipa::openapi::LicenseBuilder::new()
            .name("NULLRADIX License")
            .url(Some(
                "https://github.com/NULLRADIX-DEV/NULLRADIX-Harness/blob/main/LICENSE.md",
            ))
            .build(),
    );
    OpenApiRouter::with_openapi(doc).nest(API_PREFIX, v1)
}

/// Pages and theme styles may load only same-origin or inline data.
const APP_CSP: &str = concat!(
    "frame-ancestors 'none'; object-src 'none'; base-uri 'self'; ",
    "img-src 'self' data:; font-src 'self' data:; ",
    "style-src 'self' 'unsafe-inline'; connect-src 'self'; media-src 'self'"
);

pub fn router(state: ApiState) -> Router {
    let (router, _) = routes().split_for_parts();
    router
        .fallback(fallback)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::authenticate,
        ))
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
                .layer(TraceLayer::new_for_http().make_span_with(request_span))
                .layer(PropagateRequestIdLayer::x_request_id())
                .layer(security_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
                .layer(security_header(header::X_FRAME_OPTIONS, "DENY"))
                .layer(security_header(header::REFERRER_POLICY, "no-referrer"))
                .layer(security_header(header::CONTENT_SECURITY_POLICY, APP_CSP))
                .layer(middleware::from_fn(web::no_store_for_api))
                .layer(middleware::from_fn_with_state(
                    state.allowed_hosts.clone(),
                    host::check_host,
                )),
        )
        .layer(middleware::from_fn(body_guard::buffer_small_body))
        .with_state(state)
}

fn security_header(
    name: header::HeaderName,
    value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
}

async fn fallback(State(state): State<ApiState>, request: axum::extract::Request) -> Response {
    match &state.web_dir {
        Some(dir) if !web::is_api_path(request.uri().path()) => web::serve(dir, request).await,
        _ => route_not_found().into_response(),
    }
}

fn route_not_found() -> ApiError {
    AppError::not_found("route-not-found", "no such route").into()
}

async fn method_not_allowed() -> Response {
    problem::response(
        StatusCode::METHOD_NOT_ALLOWED,
        "method-not-allowed",
        "method not allowed on this route",
    )
}

fn request_span<B>(request: &Request<B>) -> tracing::Span {
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    tracing::info_span!(
        "request",
        method = %request.method(),
        path = %request.uri().path(),
        request_id,
    )
}

/// OpenAPI document of the whole API.
pub fn openapi_json() -> Result<String, serde_json::Error> {
    routes().split_for_parts().1.to_pretty_json()
}

pub async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
}

#[cfg(test)]
pub(crate) mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{HeaderValue, header};
    use nullradix_app::SystemClock;
    use nullradix_app::UuidV7Generator;
    use nullradix_app::auth::Secret;
    use nullradix_store::Store;
    use tower::ServiceExt;

    use super::*;

    pub(crate) struct TestApi {
        pub(crate) router: Router,
        pub(crate) token: Secret,
        pub(crate) auth: Arc<AuthService>,
        pub(crate) projects: Arc<ProjectService>,
        pub(crate) boards: Arc<BoardService>,
        pub(crate) runners: Arc<RunnerService>,
        pub(crate) person: Arc<nullradix_domain::Actor>,
        pub(crate) changes: Arc<dyn ChangeFeed>,
        pub(crate) shutdown: CancellationToken,
        pub(crate) accounts: Arc<AccountService>,
        pub(crate) pauses: Arc<AccountPauses>,
        pub(crate) logins: Arc<LoginService>,
        pub(crate) fetcher: Arc<crate::theme_catalog::fake::Fetcher>,
        pub(crate) store: Arc<Store>,
        pub(crate) slots: Arc<crate::claude_theme::fake::Slots>,
        pub(crate) _accounts_dir: tempfile::TempDir,
    }

    pub(crate) async fn test_api() -> TestApi {
        test_api_with_web(None).await
    }

    type TicketServices = (
        Arc<TicketService>,
        Arc<CommentService>,
        Arc<QuestionService>,
        Arc<WorktreeService>,
        Arc<NoteService>,
    );

    fn ticket_services(
        store: &Arc<Store>,
        runners: &Arc<RunnerService>,
        signal: &ChangeSignal,
    ) -> TicketServices {
        let tickets = Arc::new(TicketService::new(
            store.clone(),
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let comments = Arc::new(CommentService::new(
            store.clone(),
            store.clone(),
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let questions = Arc::new(QuestionService::new(
            store.clone(),
            store.clone(),
            store.clone(),
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let worktrees = Arc::new(WorktreeService::new(
            tickets.clone(),
            store.clone(),
            store.clone(),
            runners.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let notes = Arc::new(NoteService::new(
            store.clone(),
            tickets.clone(),
            store.clone(),
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        (tickets, comments, questions, worktrees, notes)
    }

    /// The Claude config folder the test runner reads transcripts below.
    pub(crate) fn transcript_root() -> PathBuf {
        std::env::temp_dir()
            .join("nullradix-api-tests")
            .join("claude")
    }

    async fn test_runners(
        store: &Arc<Store>,
        signal: &ChangeSignal,
    ) -> (Arc<nullradix_runner::LocalRunner>, Arc<RunnerService>) {
        let runner = Arc::new(
            nullradix_runner::LocalRunner::new(
                std::env::temp_dir().join("nullradix-api-tests"),
                nullradix_runner::SupervisorCommand {
                    program: PathBuf::from("nullradix-missing-supervisor"),
                    args: Vec::new(),
                },
            )
            .with_transcript_roots(vec![transcript_root()]),
        );
        let runners = RunnerService::start(
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

    fn session_services(
        store: &Arc<Store>,
        runners: &Arc<RunnerService>,
        signal: &ChangeSignal,
    ) -> (Arc<SessionService>, Arc<FavoriteService>, Arc<HookService>) {
        let favorites = Arc::new(FavoriteService::new(
            store.clone(),
            runners.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let sessions = Arc::new(SessionService::new(
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

    /// A logged in account with a fixed answer from the usage endpoint.
    struct FakeClaude;

    #[async_trait::async_trait]
    impl nullradix_app::usage::ClaudeLogins for FakeClaude {
        async fn read(
            &self,
            _dir: Option<&nullradix_domain::ConfigDir>,
        ) -> nullradix_app::usage::LoginRead {
            nullradix_app::usage::LoginRead::Found(nullradix_app::usage::ClaudeLogin {
                token: nullradix_app::secrets::SecretValue::new("sk-ant-oat01-api-test"),
                expires_at: None,
                plan: Some(String::from("max")),
            })
        }

        async fn email(&self, _dir: Option<&nullradix_domain::ConfigDir>) -> Option<String> {
            Some(String::from("me@example.com"))
        }

        fn folder(&self, _dir: Option<&nullradix_domain::ConfigDir>) -> String {
            String::from("/claude")
        }
    }

    #[async_trait::async_trait]
    impl nullradix_app::usage::UsageApi for FakeClaude {
        async fn fetch(
            &self,
            _token: &nullradix_app::secrets::SecretValue,
        ) -> Result<nullradix_app::usage::UsageReply, AppError> {
            use nullradix_app::usage::UsageWindow;
            Ok(nullradix_app::usage::UsageReply {
                status: 200,
                five_hour: Some(UsageWindow {
                    percent: 42,
                    resets_at: Some(nullradix_domain::Timestamp::from_millis(1_900_000_000_000)),
                }),
                seven_day: Some(UsageWindow {
                    percent: 7,
                    resets_at: None,
                }),
                retry_after: None,
            })
        }
    }

    /// A login that never ends by itself and accepts every code.
    struct IdleLogin;

    #[async_trait::async_trait]
    impl nullradix_app::login::LoginRunner for IdleLogin {
        async fn start(
            &self,
            _spec: &nullradix_app::login::LoginSpec,
        ) -> Result<nullradix_app::login::LoginProcess, AppError> {
            let (output_sender, output) = tokio::sync::mpsc::channel(1);
            let (exit_sender, exit) = tokio::sync::oneshot::channel();
            let (input, mut typed) = tokio::sync::mpsc::channel::<String>(4);
            tokio::spawn(async move { while typed.recv().await.is_some() {} });
            // killing ends the output and reports an exit, as a real process does
            let alive = std::sync::Mutex::new(Some((output_sender, exit_sender)));
            Ok(nullradix_app::login::LoginProcess {
                output,
                exit,
                input,
                kill: Box::new(move || {
                    if let Some((_, exit)) = alive.lock().unwrap().take() {
                        let _ = exit.send(None);
                    }
                }),
            })
        }
    }

    fn test_logins(accounts: &Arc<AccountService>) -> Arc<LoginService> {
        Arc::new(LoginService::new(
            Arc::new(IdleLogin),
            accounts.clone(),
            Arc::new(FakeClaude),
            Arc::new(SystemClock),
        ))
    }

    fn test_usage() -> Arc<UsageService> {
        Arc::new(UsageService::new(
            Arc::new(FakeClaude),
            Arc::new(FakeClaude),
            Arc::new(SystemClock),
        ))
    }

    fn test_accounts(
        store: &Arc<Store>,
        signal: &ChangeSignal,
    ) -> (Arc<AccountService>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let accounts = Arc::new(AccountService::new(
            store.clone(),
            Arc::new(nullradix_app::secrets::MemorySecretStore::new()),
            Arc::new(nullradix_runner::LocalAccountFolders::new(
                dir.path().join("accounts"),
            )),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        (accounts, dir)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "wires every service of the test daemon"
    )]
    pub(crate) async fn test_api_with_web(web_dir: Option<PathBuf>) -> TestApi {
        let store = Arc::new(Store::in_memory().await.unwrap());
        let auth = Arc::new(AuthService::new(
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
        ));
        let token = auth.local_token(None).await.unwrap();
        let signal = ChangeSignal::new();
        let shutdown = CancellationToken::new();
        let projects = Arc::new(ProjectService::new(
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let (accounts, accounts_dir) = test_accounts(&store, &signal);
        let changes: Arc<dyn ChangeFeed> = store.clone();
        let (runner, runners) = test_runners(&store, &signal).await;
        let boards = Arc::new(BoardService::new(
            store.clone(),
            store.clone(),
            store.clone(),
            runners.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let (tickets, comments, questions, worktrees, notes) =
            ticket_services(&store, &runners, &signal);
        let reviews = Arc::new(ReviewService::new(nullradix_app::review::ReviewParts {
            tickets: tickets.clone(),
            worktrees: store.clone(),
            runners: runners.clone(),
            reviews: store.clone(),
            clock: Arc::new(SystemClock),
            ids: Arc::new(UuidV7Generator),
            signal: signal.clone(),
        }));
        let person = Arc::new(
            nullradix_app::boards::local_person(
                store.as_ref(),
                "tester",
                &UuidV7Generator,
                &SystemClock,
            )
            .await
            .unwrap(),
        );
        let runs = RunService::new(
            store.clone(),
            runner.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
            std::time::Duration::from_millis(50),
        );
        let dispatcher_config = Arc::new(DispatcherConfigService::new(
            store.clone(),
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let dispatch_runs = Arc::new(DispatchRuns::new(
            runs.clone(),
            dispatcher_config.clone(),
            accounts.clone(),
            store.clone(),
            store.clone(),
            runner,
        ));
        let (sessions, favorites, hooks) = session_services(&store, &runners, &signal);
        let logins = test_logins(&accounts);
        let customizations = Arc::new(CustomizationService::new(
            store.clone(),
            Arc::new(nullradix_app::customizations::NoStandards),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let themes = Arc::new(ThemeService::new(
            store.clone(),
            Arc::new(crate::themes::fake::Files::default()),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let fetcher = Arc::new(crate::theme_catalog::fake::Fetcher::default());
        let slots = Arc::new(crate::claude_theme::fake::Slots::default());
        let catalog = Arc::new(CatalogService::new(
            store.clone(),
            fetcher.clone(),
            themes.clone(),
            Arc::new(SystemClock),
            signal.clone(),
            nullradix_domain::Reach::Public,
        ));
        let dispatcher_config = Arc::new(DispatcherConfigService::new(
            store.clone(),
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let prompts = Arc::new(DispatcherPromptService::new(
            dispatcher_config.clone(),
            tickets.clone(),
            questions.clone(),
        ));
        let usage = test_usage();
        let assembled = nullradix_app::dispatch::assemble::assemble(
            nullradix_app::dispatch::assemble::DispatcherDeps {
                config: dispatcher_config.clone(),
                states: store.clone(),
                log: store.clone(),
                accounts: accounts.clone(),
                limits: usage.clone(),
                pauses: store.clone(),
                samples: store.clone(),
                setup: Arc::new(nullradix_runner::LocalSetup),
                detached: Arc::new(nullradix_runner::LocalDetached),
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
                merge_git: Arc::new(nullradix_runner::LocalMergeGit),
                reviews: reviews.clone(),
                clock: Arc::new(SystemClock),
                ids: Arc::new(UuidV7Generator),
                signal: signal.clone(),
                local_time: Arc::new(nullradix_app::dispatch::local_time::UtcTime),
                notifier: Arc::new(nullradix_app::notifier::NoNotifier),
                interval: None,
            },
        );
        let pauses = assembled.pauses.clone();
        let router = router(ApiState {
            auth: auth.clone(),
            projects: projects.clone(),
            accounts: accounts.clone(),
            dispatcher_config: dispatcher_config.clone(),
            dispatcher_prompts: prompts,
            dispatcher: assembled.service,
            dispatch_stats: Arc::new(DispatchStatsService::new(
                store.clone(),
                dispatcher_config.clone(),
                store.clone(),
                Arc::new(SystemClock),
                Arc::new(nullradix_app::dispatch::local_time::UtcTime),
            )),
            account_pauses: pauses.clone(),
            usage,
            logins: logins.clone(),
            boards: boards.clone(),
            tickets,
            comments,
            questions,
            worktrees,
            reviews,
            decisions: assembled.decisions.clone(),
            merges: assembled.merges.clone(),
            retries: assembled.retries.clone(),
            notes,
            inbox: Arc::new(InboxService::new(
                store.clone(),
                Arc::new(SystemClock),
                signal.clone(),
            )),
            runners: runners.clone(),
            person: person.clone(),
            runs,
            dispatch_runs,
            hooks,
            sessions,
            favorites,
            themes: themes.clone(),
            claude_theme: Arc::new(nullradix_app::claude_theme::ClaudeThemeService::new(
                store.clone(),
                themes.clone(),
                store.clone(),
                slots.clone(),
                Arc::new(SystemClock),
                signal.clone(),
            )),
            catalog,
            customizations,
            changes: changes.clone(),
            signal,
            shutdown: shutdown.clone(),
            allowed_hosts: AllowedHosts::loopback_and([]),
            secure_cookies: false,
            web_dir,
        });
        TestApi {
            router,
            token,
            auth,
            projects,
            boards,
            runners,
            person,
            changes,
            shutdown,
            accounts,
            pauses,
            logins,
            fetcher,
            store: store.clone(),
            slots,
            _accounts_dir: accounts_dir,
        }
    }

    /// Redeems a handoff and returns the session cookie as `name=value`.
    pub(crate) async fn open_session(api: &TestApi) -> String {
        let code = api.auth.create_handoff().unwrap();
        let request = local(Request::get(format!(
            "/api/v1/auth/handoff?code={}",
            code.expose()
        )))
        .body(Body::empty())
        .unwrap();
        let response = api.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/");
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        cookie.split(';').next().unwrap().to_owned()
    }

    pub(crate) fn local(request: axum::http::request::Builder) -> axum::http::request::Builder {
        request.header(header::HOST, "127.0.0.1:4711")
    }

    pub(crate) fn bearer(
        request: axum::http::request::Builder,
        token: &Secret,
    ) -> axum::http::request::Builder {
        local(request).header(header::AUTHORIZATION, format!("Bearer {}", token.expose()))
    }

    pub(crate) async fn send(
        router: &Router,
        request: Request<Body>,
    ) -> (StatusCode, Option<HeaderValue>, serde_json::Value) {
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json = if body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        (status, content_type, json)
    }

    #[tokio::test]
    async fn unknown_routes_answer_with_problem_details() {
        let api = test_api().await;
        let request = bearer(Request::get("/api/v1/nope"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, content_type, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(content_type.unwrap(), "application/problem+json");
        assert_eq!(json["code"], "route-not-found");
    }

    #[tokio::test]
    async fn wrong_methods_answer_with_problem_details() {
        let api = test_api().await;
        let request = local(Request::post("/api/v1/health"))
            .body(Body::empty())
            .unwrap();
        let (status, content_type, _) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(content_type.unwrap(), "application/problem+json");
    }

    #[tokio::test]
    async fn refuses_foreign_host_names() {
        let api = test_api().await;
        let request = Request::get("/api/v1/health")
            .header(header::HOST, "attacker.example.com:4711")
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["code"], "host-not-allowed");
    }

    #[test]
    fn openapi_document_lists_routes_and_security() {
        let json = openapi_json().unwrap();
        assert!(json.contains("\"/api/v1/health\""));
        assert!(json.contains("\"/api/v1/auth/handoffs\""));
        assert!(json.contains("NULLRADIX API"));
        assert!(json.contains("\"Problem\""));
        assert!(json.contains("\"nullradix_session\""));
    }

    #[test]
    fn openapi_version_is_the_api_version() {
        let json: serde_json::Value = serde_json::from_str(&openapi_json().unwrap()).unwrap();
        assert_eq!(json["info"]["version"], API_VERSION.to_string());
    }
}
