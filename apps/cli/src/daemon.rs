use std::env;
use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use nullradix_api::{API_VERSION, AllowedHosts, ApiState, Limits};
use nullradix_app::accounts::AccountService;
use nullradix_app::auth::AuthService;
use nullradix_app::autostart::{Autostart, AutostartService, NoAutostart};
use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::claude_theme::ClaudeThemeService;
use nullradix_app::comments::CommentService;
use nullradix_app::customizations::CustomizationService;
use nullradix_app::dispatch::accounts::AccountPauses;
use nullradix_app::dispatch::assemble::{DispatcherDeps, assemble};
use nullradix_app::dispatch::control::DispatcherService;
use nullradix_app::dispatch::local_time::LocalTime;
use nullradix_app::dispatch::pass::DispatchPass;
use nullradix_app::dispatch::review_decide::ReviewDecisions;
use nullradix_app::dispatch::runs::{DispatchRuns, RunFilesPort};
use nullradix_app::dispatch::scheduler::{PASS_INTERVAL, spawn_scheduler};
use nullradix_app::dispatch::start::TicketStarter;
use nullradix_app::dispatch::stats::DispatchStatsService;
use nullradix_app::dispatcher_config::DispatcherConfigService;
use nullradix_app::dispatcher_prompts::DispatcherPromptService;
use nullradix_app::favorites::FavoriteService;
use nullradix_app::hooks::HookService;
use nullradix_app::inbox::InboxService;
use nullradix_app::login::LoginService;
use nullradix_app::notes::NoteService;
use nullradix_app::notifier::{NoNotifier, Notifier};
use nullradix_app::projects::ProjectService;
use nullradix_app::questions::QuestionService;
use nullradix_app::repositories::RunnerService;
use nullradix_app::review::{ReviewParts, ReviewService};
use nullradix_app::runs::RunService;
use nullradix_app::sessions::SessionService;
use nullradix_app::snippets::SnippetService;
use nullradix_app::theme_catalog::CatalogService;
use nullradix_app::theme_fetch::ThemeAssets;
use nullradix_app::theme_preview::ThemePreviews;
use nullradix_app::themes::ThemeService;
use nullradix_app::tickets::TicketService;
use nullradix_app::usage::UsageService;
use nullradix_app::worktrees::WorktreeService;
use nullradix_app::{ChangeSignal, Clock, SystemClock, UuidV7Generator};
use nullradix_client::{Discovery, DiscoveryFile, Spool};
use nullradix_domain::{Actor, ConfigDir, NetworkShares, Reach, Timestamp};
use nullradix_os::Paths;
use nullradix_runner::{
    CurlAssetFetcher, CurlCatalogFetcher, CurlTokenEndpoint, CurlUsageApi, LocalAccountFolders,
    LocalClaudeLogins, LocalDetached, LocalLoginRenewal, LocalLoginRunner, LocalRunner, LocalSetup,
    SupervisorCommand, TranscriptRoots,
};
use nullradix_store::Store;
use tokio::net::TcpListener;
use tokio::task;
use tokio_util::sync::CancellationToken;

use crate::claude_slots::{FolderSlots, default_claude_dir};
use crate::config::{Config, Profile};
use crate::desktop::{LoginEntry, ToastNotifier};
use crate::intake;
use crate::logging;
use crate::secrets::OsSecretStore;
use crate::themes::FolderThemes;
use crate::token;

/// Time open connections get to finish once a shutdown is requested.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
/// How often the daemon checks on active runs.
const RUN_POLL: Duration = Duration::from_millis(500);

/// Creates the daemon folders for the owner only.
fn private_roots(paths: &Paths) -> anyhow::Result<()> {
    let runs = paths.data.join("runs");
    let accounts = paths.data.join("accounts");
    for dir in [
        &paths.data,
        &paths.config,
        &paths.cache,
        &paths.logs,
        &runs,
        &accounts,
    ] {
        nullradix_os::create_data_dir(dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
    }
    Ok(())
}

/// Makes the database files private to the owner.
fn private_database(path: &Path) -> io::Result<()> {
    nullradix_os::ensure_private_file(path)?;
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        nullradix_os::tighten_private_file(Path::new(&name))?;
    }
    Ok(())
}

pub(crate) fn run(config: &Config) -> anyhow::Result<()> {
    private_roots(&config.paths)?;
    let _logging = logging::init(&config.paths.logs, &config.log_filter)?;
    let data = &config.paths.data;
    let _lock = lock_profile(data)?;
    let discovery = DiscoveryFile::in_dir(data);
    // whatever is left belongs to a dead daemon, we hold the lock now
    discovery.remove()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("nullradix")
        .build()
        .context("cannot start the async runtime")?;
    runtime.block_on(serve(config, discovery))
}

/// Binds the loopback address and announces it in the discovery file.
async fn listen(config: &Config, discovery: &DiscoveryFile) -> anyhow::Result<TcpListener> {
    if !config.address.ip().is_loopback() {
        bail!(
            "refusing to listen on {}: credentials would travel unencrypted, use a loopback address",
            config.address
        );
    }
    let listener = TcpListener::bind(config.address)
        .await
        .with_context(|| format!("cannot listen on {}", config.address))?;
    let address = listener.local_addr()?;
    let record = Discovery {
        pid: std::process::id(),
        address,
        version: String::from(env!("CARGO_PKG_VERSION")),
        api_version: API_VERSION,
        started_at: SystemClock.now().as_millis(),
    };
    let writer = discovery.clone();
    task::spawn_blocking(move || writer.write(&record)).await??;
    tracing::info!(
        %address,
        profile = ?config.profile,
        data = %config.paths.data.display(),
        "daemon started"
    );
    Ok(listener)
}

#[allow(clippy::too_many_lines, reason = "wires every service of the daemon")]
async fn serve(config: &Config, discovery: DiscoveryFile) -> anyhow::Result<()> {
    let database = config.paths.data.join("nullradix.db");
    private_database(&database).context("cannot secure the database file")?;
    let store = Store::open(&database)
        .await
        .context("cannot open the database")?;
    let auth = Arc::new(AuthService::new(
        Arc::new(store.clone()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
    ));
    ensure_local_token(config, &auth).await?;
    let pruned = auth.prune_expired_sessions().await?;
    tracing::debug!(pruned, "expired sessions removed");
    let listener = listen(config, &discovery).await?;
    let signal = ChangeSignal::new();
    let shutdown = CancellationToken::new();
    let projects = Arc::new(ProjectService::new(
        Arc::new(store.clone()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let dispatcher_config = dispatcher_config_service(&store, &signal);
    dispatcher_config
        .sync_hash()
        .await
        .context("cannot record the dispatcher configuration")?;
    let account_services = start_accounts(config, &store, &signal).await?;
    let roots =
        start_transcript_roots(config, &account_services.accounts, &signal, &shutdown).await?;
    let (runner, runners) = start_runners(config, &store, &signal, roots).await?;
    let board = start_board(&store, &runners, &signal).await?;
    let (runs, dispatch_runs) = start_runs(
        &store,
        runner,
        &dispatcher_config,
        &account_services.accounts,
        &signal,
    )
    .await?;
    let (themes, catalog, customizations, snippets) = start_extensions(config, &store, &signal);
    let theme_assets = start_theme_assets(config, &themes);
    let theme_previews = start_theme_previews(config, &catalog).await;
    let claude_theme = start_claude_theme(config, &store, &themes, &signal);
    if let Err(error) = claude_theme.reconcile().await {
        tracing::warn!(%error, "cannot sync the Claude theme");
    }
    let autostart = start_autostart(config);
    autostart.refresh().await;
    let (favorites, sessions) = start_sessions(&store, &runners, &signal);
    let hooks = start_hooks(config, &store, &sessions, &shutdown);
    let dispatcher_prompts = dispatcher_prompt_service(&dispatcher_config, &board);
    let (dispatcher, account_pauses, deciding, dispatching) = start_dispatcher(
        &store,
        &signal,
        &shutdown,
        DispatcherParts {
            config: dispatcher_config.clone(),
            accounts: account_services.accounts.clone(),
            usage: account_services.usage.clone(),
            runners: runners.clone(),
            runs: runs.clone(),
            dispatch: dispatch_runs.clone(),
            prompts: dispatcher_prompts.clone(),
            board: &board,
            notifier: start_notifier(config),
            plugin: config.profile.plugin_id(),
            shares: network_shares(config),
        },
    );
    let limits = Limits::default();
    let state = ApiState {
        auth,
        projects,
        accounts: account_services.accounts,
        dispatcher_config: dispatcher_config.clone(),
        dispatcher_prompts,
        dispatcher: dispatcher.clone(),
        dispatch_stats: Arc::new(DispatchStatsService::new(
            Arc::new(store.clone()),
            dispatcher_config.clone(),
            Arc::new(store.clone()),
            Arc::new(SystemClock),
            Arc::new(OsLocalTime),
        )),
        account_pauses,
        usage: account_services.usage,
        logins: account_services.logins.clone(),
        boards: board.boards,
        tickets: board.tickets,
        comments: board.comments,
        questions: board.questions,
        worktrees: board.worktrees,
        reviews: board.reviews,
        decisions: deciding.decisions,
        merges: deciding.merges,
        retries: deciding.retries,
        notes: board.notes,
        inbox: board.inbox,
        runners,
        person: board.person,
        runs,
        dispatch_runs,
        hooks,
        sessions,
        favorites,
        themes,
        theme_assets,
        theme_previews,
        catalog,
        claude_theme,
        autostart,
        customizations,
        snippets,
        changes: Arc::new(store.clone()),
        signal,
        shutdown: shutdown.clone(),
        allowed_hosts: AllowedHosts::loopback_and(config.allowed_hosts.iter().cloned()),
        limits: limits.clone(),
        secure_cookies: false,
        web_dir: config.web_dir.clone(),
    };
    let trigger = shutdown.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        trigger.cancel();
    });
    let http = nullradix_api::serve(
        listener,
        nullradix_api::router(state),
        &limits,
        shutdown.clone().cancelled_owned(),
    );
    let served = tokio::select! {
        result = http => result.context("the HTTP server failed"),
        () = async {
            shutdown.cancelled().await;
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        } => {
            tracing::warn!("open connections did not close in time");
            Ok(())
        }
    };
    dispatching.stop().await;
    account_services.logins.close();
    store.close().await;
    match task::spawn_blocking(move || discovery.remove()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(error = %error, "cannot remove the discovery file"),
        Err(error) => tracing::warn!(%error, "cannot remove the discovery file"),
    }
    served?;
    tracing::info!("daemon stopped");
    Ok(())
}

/// Local time as the machine tells it, per moment because daylight saving moves it.
struct OsLocalTime;

/// The failure to read the offset is reported once per process.
static OFFSET_WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[async_trait::async_trait]
impl LocalTime for OsLocalTime {
    async fn offset_minutes(&self, at: Timestamp) -> i32 {
        self.offsets(&[at]).await.first().copied().unwrap_or(0)
    }

    async fn offsets(&self, moments: &[Timestamp]) -> Vec<i32> {
        let millis: Vec<i64> = moments.iter().map(|moment| moment.as_millis()).collect();
        let found = task::spawn_blocking(move || {
            let zone = nullradix_os::LocalZone::system()?;
            Some(
                millis
                    .iter()
                    .map(|at| zone.offset_minutes(*at))
                    .collect::<Vec<_>>(),
            )
        })
        .await
        .ok()
        .flatten();
        let Some(found) = found else {
            if !OFFSET_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                tracing::warn!("cannot read the local time zone, local times are shown in UTC");
            }
            return vec![0; moments.len()];
        };
        found
            .into_iter()
            .map(|offset| offset.unwrap_or(0))
            .collect()
    }
}

/// What the dispatcher needs from the other services.
struct DispatcherParts<'a> {
    config: Arc<DispatcherConfigService>,
    accounts: Arc<AccountService>,
    usage: Arc<UsageService>,
    runners: Arc<RunnerService>,
    runs: Arc<RunService>,
    dispatch: Arc<DispatchRuns>,
    prompts: Arc<DispatcherPromptService>,
    board: &'a BoardServices,
    notifier: Arc<dyn Notifier>,
    plugin: String,
    shares: NetworkShares,
}

/// What runs for the dispatcher and must end before the store closes.
struct Dispatching {
    scheduler: task::JoinHandle<()>,
    pass: Arc<DispatchPass>,
    starter: Arc<TicketStarter>,
}

impl Dispatching {
    /// Waits for the passes under way, then stops the setups and waits for them.
    async fn stop(self) {
        if tokio::time::timeout(SHUTDOWN_GRACE, self.scheduler)
            .await
            .is_err()
        {
            tracing::warn!("the dispatcher pass did not end in time");
        }
        // a pass that outlived its caller may still start setups
        if tokio::time::timeout(SHUTDOWN_GRACE, self.pass.settle())
            .await
            .is_err()
        {
            tracing::warn!("a dispatcher pass did not end in time");
        }
        self.starter.shutdown(SHUTDOWN_GRACE).await;
    }
}

/// What decides over reviews and merges for the API.
struct Deciding {
    decisions: Arc<ReviewDecisions>,
    merges: Arc<nullradix_app::dispatch::merge_jobs::MergeQueue>,
    retries: Arc<nullradix_app::dispatch::merge_retry::MergeRetry>,
}

/// The dispatcher and its beat: a pass every minute until the daemon stops.
fn start_dispatcher(
    store: &Store,
    signal: &ChangeSignal,
    shutdown: &CancellationToken,
    parts: DispatcherParts<'_>,
) -> (
    Arc<DispatcherService>,
    Arc<AccountPauses>,
    Deciding,
    Dispatching,
) {
    let dispatcher = assemble(DispatcherDeps {
        config: parts.config,
        states: Arc::new(store.clone()),
        log: Arc::new(store.clone()),
        accounts: parts.accounts,
        limits: parts.usage,
        pauses: Arc::new(store.clone()),
        samples: Arc::new(store.clone()),
        setup: Arc::new(LocalSetup::default().with_network_shares(parts.shares.clone())),
        detached: Arc::new(
            LocalDetached::new(Some(parts.plugin.clone()))
                .with_network_shares(parts.shares.clone()),
        ),
        tracking: Arc::new(store.clone()),
        questions: parts.board.questions.clone(),
        actors: Arc::new(store.clone()),
        boards: Arc::new(store.clone()),
        runners: parts.runners,
        runs: parts.runs,
        dispatch: parts.dispatch,
        prompts: parts.prompts,
        tickets: parts.board.tickets.clone(),
        comments: parts.board.comments.clone(),
        worktrees: parts.board.worktrees.clone(),
        worktree_repository: Arc::new(store.clone()),
        merge_jobs: Arc::new(store.clone()),
        merge_git: Arc::new(
            nullradix_runner::LocalMergeGit::new(Some(parts.plugin))
                .with_network_shares(parts.shares.clone()),
        ),
        reviews: parts.board.reviews.clone(),
        clock: Arc::new(SystemClock),
        ids: Arc::new(UuidV7Generator),
        signal: signal.clone(),
        local_time: Arc::new(OsLocalTime),
        notifier: parts.notifier,
        interval: Some(PASS_INTERVAL),
    });
    let scheduler = spawn_scheduler(
        dispatcher.pass.clone(),
        PASS_INTERVAL,
        shutdown.clone().cancelled_owned(),
    );
    let dispatching = Dispatching {
        scheduler,
        pass: dispatcher.pass,
        starter: dispatcher.starter,
    };
    (
        dispatcher.service,
        dispatcher.pauses,
        Deciding {
            decisions: dispatcher.decisions,
            merges: dispatcher.merges,
            retries: dispatcher.retries,
        },
        dispatching,
    )
}

fn dispatcher_config_service(store: &Store, signal: &ChangeSignal) -> Arc<DispatcherConfigService> {
    Arc::new(DispatcherConfigService::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ))
}

fn dispatcher_prompt_service(
    config: &Arc<DispatcherConfigService>,
    board: &BoardServices,
) -> Arc<DispatcherPromptService> {
    Arc::new(DispatcherPromptService::new(
        config.clone(),
        board.tickets.clone(),
        board.questions.clone(),
    ))
}

/// Accounts with their secrets, usage limits and logins.
struct AccountServices {
    accounts: Arc<AccountService>,
    usage: Arc<UsageService>,
    logins: Arc<LoginService>,
}

async fn start_accounts(
    config: &Config,
    store: &Store,
    signal: &ChangeSignal,
) -> anyhow::Result<AccountServices> {
    let accounts = Arc::new(AccountService::new(
        Arc::new(store.clone()),
        Arc::new(OsSecretStore::open(config).await?),
        Arc::new(account_folders(config)),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let files = Arc::new(claude_logins(config));
    let renewal = LocalLoginRenewal::new(
        files.clone(),
        Arc::new(CurlTokenEndpoint::anthropic()),
        Arc::new(SystemClock),
    );
    let usage = Arc::new(
        UsageService::new(
            files.clone(),
            Arc::new(CurlUsageApi::anthropic()),
            Arc::new(SystemClock),
        )
        .with_renewal(Arc::new(renewal)),
    );
    let logins = Arc::new(LoginService::new(
        Arc::new(login_runner(config)),
        accounts.clone(),
        files,
        Arc::new(SystemClock),
    ));
    Ok(AccountServices {
        accounts,
        usage,
        logins,
    })
}

/// The login runner; a home override gives it the home and default folder the usage reading uses.
fn login_runner(config: &Config) -> LocalLoginRunner {
    if config.isolated {
        LocalLoginRunner::claude()
            .with_home(Some(config.paths.data.clone()))
            .with_default_folder(Some(config.paths.data.join(".claude")))
            .with_network_shares(network_shares(config))
    } else {
        LocalLoginRunner::claude().with_network_shares(network_shares(config))
    }
}

/// Hosts allowed for network paths of repositories, run folders and account folders.
fn network_shares(config: &Config) -> NetworkShares {
    NetworkShares::new(&config.network_shares)
}

/// New accounts get folders in the data folder; a home override also moves `~` there.
fn account_folders(config: &Config) -> LocalAccountFolders {
    let folders = LocalAccountFolders::new(config.paths.data.join("accounts"))
        .with_network_shares(network_shares(config));
    if config.isolated {
        folders.with_home(Some(config.paths.data.clone()))
    } else {
        folders
    }
}

/// Claude's login files: the real ones, or none of them when a home override isolates the run.
fn claude_logins(config: &Config) -> LocalClaudeLogins {
    let logins = if config.isolated {
        LocalClaudeLogins::new(Some(config.paths.data.clone()), None)
    } else {
        LocalClaudeLogins::from_environment()
    };
    logins.with_network_shares(network_shares(config))
}

/// Favorites and session details, both read from what the runners found.
fn start_sessions(
    store: &Store,
    runners: &Arc<RunnerService>,
    signal: &ChangeSignal,
) -> (Arc<FavoriteService>, Arc<SessionService>) {
    let favorites = Arc::new(FavoriteService::new(
        Arc::new(store.clone()),
        runners.clone(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let sessions = Arc::new(SessionService::new(
        Arc::new(store.clone()),
        runners.clone(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    (favorites, sessions)
}

/// Where the proxy keeps what it loaded for themes; only a copy, so it lives in the cache folder.
fn theme_asset_cache(config: &Config) -> std::path::PathBuf {
    config.paths.cache.join("theme-assets")
}

/// The asset proxy: public https only, whatever the setting for local catalogs says.
fn start_theme_assets(config: &Config, themes: &Arc<ThemeService>) -> Arc<ThemeAssets> {
    Arc::new(ThemeAssets::new(
        themes.clone(),
        Arc::new(CurlAssetFetcher::new()),
        Arc::new(crate::theme_assets::ZipFiles),
        Arc::new(crate::theme_assets::FolderAssets::new(theme_asset_cache(
            config,
        ))),
    ))
}

/// Previews are only a copy and live in the cache folder.
fn theme_preview_cache(config: &Config) -> std::path::PathBuf {
    config.paths.cache.join("theme-previews")
}

/// The preview proxy has the same reach as the catalog.
async fn start_theme_previews(
    config: &Config,
    catalog: &Arc<CatalogService>,
) -> Arc<ThemePreviews> {
    let folder = crate::theme_assets::FolderPreviews::new(theme_preview_cache(config));
    // A stop in the middle of a removal leaves a folder of a missing source.
    match catalog.sources().await {
        Ok(list) => {
            let ids: Vec<_> = list
                .sources
                .iter()
                .map(|source| source.id().clone())
                .collect();
            if let Err(error) = folder.prune(&ids).await {
                tracing::warn!(%error, "cannot remove the previews of removed sources");
            }
        }
        Err(error) => tracing::warn!(%error, "cannot list the sources to prune their previews"),
    }
    Arc::new(ThemePreviews::new(
        catalog.clone(),
        Arc::new(CurlCatalogFetcher::new(catalog_reach(config))),
        Arc::new(folder),
    ))
}

fn catalog_reach(config: &Config) -> Reach {
    if config.theme_catalog_local {
        Reach::Anywhere
    } else {
        Reach::Public
    }
}

/// Themes from the data folder and the customizations in the database.
fn start_extensions(
    config: &Config,
    store: &Store,
    signal: &ChangeSignal,
) -> (
    Arc<ThemeService>,
    Arc<CatalogService>,
    Arc<CustomizationService>,
    Arc<SnippetService>,
) {
    let themes = Arc::new(ThemeService::new(
        Arc::new(store.clone()),
        Arc::new(FolderThemes::new(config.paths.themes()).with_cache(theme_asset_cache(config))),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let reach = catalog_reach(config);
    let catalog = Arc::new(CatalogService::new(
        Arc::new(store.clone()),
        Arc::new(CurlCatalogFetcher::new(reach)),
        themes.clone(),
        Arc::new(SystemClock),
        signal.clone(),
        reach,
    ));
    let customizations = Arc::new(CustomizationService::new(
        Arc::new(store.clone()),
        Arc::new(crate::plugin::ShippedStandards),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let snippets = Arc::new(SnippetService::new(
        Arc::new(store.clone()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    (themes, catalog, customizations, snippets)
}

/// Toasts for notices; a home override, as tests use, shows none.
fn start_notifier(config: &Config) -> Arc<dyn Notifier> {
    if config.isolated {
        Arc::new(NoNotifier)
    } else {
        Arc::new(ToastNotifier::new())
    }
}

/// The login entry of the real user; an isolated daemon must never touch it.
fn start_autostart(config: &Config) -> Arc<AutostartService> {
    let port: Arc<dyn Autostart> = match LoginEntry::new(config.profile) {
        Some(entry) if !config.isolated => Arc::new(entry),
        _ => Arc::new(NoAutostart),
    };
    Arc::new(AutostartService::new(port))
}

/// The Claude part of themes goes into the default Claude folder and every account's folder.
fn start_claude_theme(
    config: &Config,
    store: &Store,
    themes: &Arc<ThemeService>,
    signal: &ChangeSignal,
) -> Arc<ClaudeThemeService> {
    let home = config.isolated.then_some(config.paths.data.as_path());
    Arc::new(ClaudeThemeService::new(
        Arc::new(store.clone()),
        themes.clone(),
        Arc::new(store.clone()),
        Arc::new(
            FolderSlots::new(config.paths.themes(), default_claude_dir(home))
                .with_network_shares(network_shares(config)),
        ),
        Arc::new(SystemClock),
        signal.clone(),
    ))
}

struct BoardServices {
    boards: Arc<BoardService>,
    tickets: Arc<TicketService>,
    comments: Arc<CommentService>,
    questions: Arc<QuestionService>,
    worktrees: Arc<WorktreeService>,
    reviews: Arc<ReviewService>,
    notes: Arc<NoteService>,
    inbox: Arc<InboxService>,
    /// The person every token and browser session acts for.
    person: Arc<Actor>,
}

async fn start_board(
    store: &Store,
    runners: &Arc<RunnerService>,
    signal: &ChangeSignal,
) -> anyhow::Result<BoardServices> {
    let person = local_person(store, &person_name(), &UuidV7Generator, &SystemClock)
        .await
        .context("cannot record the local person; NULLRADIX_PERSON takes up to 100 characters")?;
    tracing::debug!(person = %person.id(), "acting person");
    let boards = Arc::new(BoardService::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::clone(runners),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let tickets = Arc::new(TicketService::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let comments = Arc::new(CommentService::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let questions = Arc::new(QuestionService::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let worktrees = Arc::new(WorktreeService::new(
        Arc::clone(&tickets),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::clone(runners),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let reviews = Arc::new(ReviewService::new(ReviewParts {
        tickets: Arc::clone(&tickets),
        worktrees: Arc::new(store.clone()),
        runners: Arc::clone(runners),
        reviews: Arc::new(store.clone()),
        clock: Arc::new(SystemClock),
        ids: Arc::new(UuidV7Generator),
        signal: signal.clone(),
    }));
    let notes = Arc::new(NoteService::new(
        Arc::new(store.clone()),
        Arc::clone(&tickets),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    Ok(BoardServices {
        boards,
        tickets,
        comments,
        questions,
        worktrees,
        reviews,
        notes,
        inbox: Arc::new(InboxService::new(
            Arc::new(store.clone()),
            Arc::new(SystemClock),
            signal.clone(),
        )),
        person: Arc::new(person),
    })
}

/// `NULLRADIX_PERSON`, else the account name of the operating system.
fn person_name() -> String {
    ["NULLRADIX_PERSON", "USERNAME", "USER"]
        .iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.trim().is_empty()))
        .unwrap_or_else(|| String::from("owner"))
}
/// Sets the config folders whose transcripts may be read: Claude's own and one per account.
async fn refresh_roots(
    folders: &LocalAccountFolders,
    accounts: &AccountService,
    roots: &TranscriptRoots,
) -> anyhow::Result<()> {
    let dirs: Vec<ConfigDir> = accounts
        .list()
        .await
        .context("cannot list the accounts")?
        .iter()
        .filter_map(|account| account.config_dir().cloned())
        .collect();
    roots.replace(folders.transcript_roots(nullradix_os::claude_config_dirs(), &dirs));
    Ok(())
}

/// The config folders whose transcripts may be read, kept current as accounts change.
async fn start_transcript_roots(
    config: &Config,
    accounts: &Arc<AccountService>,
    signal: &ChangeSignal,
    shutdown: &CancellationToken,
) -> anyhow::Result<TranscriptRoots> {
    let roots = TranscriptRoots::default();
    let folders = account_folders(config);
    refresh_roots(&folders, accounts, &roots).await?;
    keep_roots_current(
        folders,
        accounts.clone(),
        signal,
        roots.clone(),
        shutdown.clone(),
    );
    Ok(roots)
}

/// Reads the account folders again after every change, so a new account's transcripts open at once.
fn keep_roots_current(
    folders: LocalAccountFolders,
    accounts: Arc<AccountService>,
    signal: &ChangeSignal,
    roots: TranscriptRoots,
    shutdown: CancellationToken,
) {
    let mut changes = signal.subscribe();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                outcome = changes.changed() => {
                    if outcome.is_err() {
                        return;
                    }
                }
                () = shutdown.cancelled() => return,
            }
            if let Err(error) = refresh_roots(&folders, &accounts, &roots).await {
                tracing::warn!(%error, "transcript folders not refreshed");
            }
        }
    });
}

/// The runner of this profile; its worktrees get the plugin of the profile.
fn local_runner(runs: PathBuf, supervisor: SupervisorCommand, profile: Profile) -> LocalRunner {
    LocalRunner::new(runs, supervisor, Some(profile.plugin_id()))
}

/// The runner on this machine, recorded as the local runner; runs start under this binary.
async fn start_runners(
    config: &Config,
    store: &Store,
    signal: &ChangeSignal,
    roots: TranscriptRoots,
) -> anyhow::Result<(Arc<LocalRunner>, Arc<RunnerService>)> {
    let supervisor = SupervisorCommand {
        program: std::env::current_exe().context("cannot locate the nullradix binary")?,
        args: vec![String::from("run-supervisor")],
    };
    let runner = Arc::new(
        local_runner(config.paths.data.join("runs"), supervisor, config.profile)
            .with_live_transcript_roots(roots)
            .with_network_shares(network_shares(config)),
    );
    let runners = RunnerService::start(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        runner.clone(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    )
    .await
    .context("cannot record the local runner")?;
    tracing::debug!(runner = %runners.local().id(), "local runner");
    Ok((runner, Arc::new(runners)))
}

/// Run services on the local runner, watching runs that outlived the previous daemon.
async fn start_runs(
    store: &Store,
    runner: Arc<LocalRunner>,
    dispatcher_config: &Arc<DispatcherConfigService>,
    accounts: &Arc<AccountService>,
    signal: &ChangeSignal,
) -> anyhow::Result<(Arc<RunService>, Arc<DispatchRuns>)> {
    let runs = RunService::new(
        Arc::new(store.clone()),
        runner.clone(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
        RUN_POLL,
    );
    let recovered = runs.recover().await.context("cannot recover active runs")?;
    if recovered > 0 {
        tracing::info!(recovered, "watching runs from before the restart");
    }
    let files: Arc<dyn RunFilesPort> = runner;
    let dispatch = Arc::new(DispatchRuns::new(
        runs.clone(),
        dispatcher_config.clone(),
        accounts.clone(),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        files,
    ));
    Ok((runs, dispatch))
}

/// Hook service; what hooks spooled while the daemon was away is taken in alongside.
fn start_hooks(
    config: &Config,
    store: &Store,
    sessions: &Arc<SessionService>,
    shutdown: &CancellationToken,
) -> Arc<HookService> {
    let hooks = Arc::new(
        HookService::new(Arc::new(store.clone()), Arc::new(SystemClock))
            .with_sessions(Arc::clone(sessions)),
    );
    let spool = Spool::in_dir(&config.paths.data);
    intake::start(Arc::clone(&hooks), spool, shutdown.clone());
    hooks
}

/// Makes sure the token file holds a token the daemon accepts.
async fn ensure_local_token(config: &Config, auth: &AuthService) -> anyhow::Result<()> {
    let path = token::path(&config.paths);
    let reader = path.clone();
    let existing = task::spawn_blocking(move || token::read(&reader))
        .await?
        .with_context(|| format!("cannot read {}", path.display()))?;
    let secret = auth.local_token(existing.clone()).await?;
    if existing.as_ref() != Some(&secret) {
        let writer = path.clone();
        task::spawn_blocking(move || token::write(&writer, &secret))
            .await?
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(())
}

/// Holds an exclusive lock so only one daemon serves a profile.
fn lock_profile(data: &Path) -> anyhow::Result<File> {
    let path = data.join("daemon.lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => {
            bail!("another daemon is already running for this profile")
        }
        Err(TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("cannot lock {}", path.display()))
        }
    }
}

async fn shutdown_signal() {
    let interrupt = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "cannot listen for ctrl-c");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(windows)]
    let terminate = async {
        use tokio::signal::windows::{ctrl_close, ctrl_shutdown};
        match (ctrl_close(), ctrl_shutdown()) {
            (Ok(mut close), Ok(mut shutdown)) => {
                tokio::select! {
                    _ = close.recv() => {},
                    _ = shutdown.recv() => {},
                }
            }
            (Err(error), _) | (_, Err(error)) => {
                tracing::error!(%error, "cannot listen for console close events");
                std::future::pending::<()>().await;
            }
        }
    };

    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
    tracing::info!("shutdown requested");
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    use nullradix_app::runs::RunnerPort;
    use nullradix_app::secrets::MemorySecretStore;
    use nullradix_app::worktrees::NewWorktree;

    use super::*;

    async fn accounts(dir: &Path, signal: &ChangeSignal) -> Arc<AccountService> {
        let store = Store::in_memory().await.unwrap();
        Arc::new(AccountService::new(
            Arc::new(store),
            Arc::new(MemorySecretStore::new()),
            Arc::new(LocalAccountFolders::new(dir.join("accounts"))),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ))
    }

    #[tokio::test]
    async fn the_folder_of_an_account_added_after_the_start_is_read_at_once() {
        let temp = tempfile::tempdir().unwrap();
        let signal = ChangeSignal::new();
        let accounts = accounts(temp.path(), &signal).await;
        let folders = LocalAccountFolders::new(temp.path().join("accounts"));
        let roots = TranscriptRoots::default();
        refresh_roots(&folders, &accounts, &roots).await.unwrap();
        let before = roots.get();
        let shutdown = CancellationToken::new();
        keep_roots_current(
            folders,
            accounts.clone(),
            &signal,
            roots.clone(),
            shutdown.clone(),
        );

        let max = accounts.create("Max", None, None).await.unwrap();
        let folder = PathBuf::from(max.config_dir().unwrap().as_str());
        for _ in 0..200 {
            if roots.get().contains(&folder) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(roots.get().contains(&folder));
        assert!(!before.contains(&folder));
        shutdown.cancel();
    }

    #[tokio::test]
    async fn a_runner_of_the_dev_profile_enables_the_dev_plugin_in_its_worktrees() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arches");
        fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            let done = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(done.status.success());
        }
        let supervisor = SupervisorCommand {
            program: PathBuf::from("nullradix-missing-supervisor"),
            args: Vec::new(),
        };
        let runner = local_runner(temp.path().join("runs"), supervisor, Profile::Dev);
        let path = temp.path().join("arches-worktrees").join("ARC-0001");
        let text = |path: &Path| path.to_string_lossy().replace('\\', "/");

        runner
            .create_worktree(&NewWorktree {
                cwd: text(&root),
                root: text(&root),
                branch: String::from("ticket/a"),
                path: text(&path),
                target: None,
            })
            .await
            .unwrap();

        let settings = fs::read_to_string(path.join(".claude/settings.local.json")).unwrap();
        assert!(
            settings.contains("nullradix-dev@nullradix-dev-local"),
            "{settings}"
        );
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_roots_are_private_and_older_open_ones_are_tightened() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::below(temp.path());
        for old in [&paths.data, &paths.logs, &paths.data.join("runs")] {
            fs::create_dir_all(old).unwrap();
            set_mode(old, 0o755);
        }
        private_roots(&paths).unwrap();
        for dir in [&paths.data, &paths.config, &paths.cache, &paths.logs] {
            assert_eq!(mode_of(dir), 0o700, "{}", dir.display());
        }
        assert_eq!(mode_of(&paths.data.join("runs")), 0o700);
        assert_eq!(mode_of(&paths.data.join("accounts")), 0o700);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_new_database_and_its_wal_files_are_private() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nullradix.db");
        private_database(&path).unwrap();
        assert_eq!(mode_of(&path), 0o600);
        let store = Store::open(&path).await.unwrap();
        for suffix in ["", "-wal", "-shm"] {
            let file = PathBuf::from(format!("{}{suffix}", path.display()));
            assert_eq!(mode_of(&file), 0o600, "{}", file.display());
        }
        store.close().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_database_files_of_an_older_install_are_tightened() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nullradix.db");
        // an open store keeps its wal files, like a killed daemon
        let store = Store::open(&path).await.unwrap();
        let files: Vec<PathBuf> = ["", "-wal", "-shm"]
            .iter()
            .map(|suffix| PathBuf::from(format!("{}{suffix}", path.display())))
            .collect();
        for file in &files {
            assert!(file.exists(), "{}", file.display());
            set_mode(file, 0o644);
        }
        private_database(&path).unwrap();
        for file in &files {
            assert_eq!(mode_of(file), 0o600, "{}", file.display());
        }
        store.close().await;
    }
}
