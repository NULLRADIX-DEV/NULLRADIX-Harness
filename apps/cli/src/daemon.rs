use std::env;
use std::fs::{self, File, TryLockError};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use nullradix_api::{API_VERSION, AllowedHosts, ApiState};
use nullradix_app::accounts::AccountService;
use nullradix_app::auth::AuthService;
use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::comments::CommentService;
use nullradix_app::customizations::{CustomizationService, NoStandards};
use nullradix_app::dispatch::assemble::{DispatcherDeps, assemble};
use nullradix_app::dispatch::control::DispatcherService;
use nullradix_app::dispatch::local_time::{LocalTime, minute_with_offset};
use nullradix_app::dispatch::pass::DispatchPass;
use nullradix_app::dispatch::runs::{DispatchRuns, RunFilesPort};
use nullradix_app::dispatch::scheduler::{PASS_INTERVAL, spawn_scheduler};
use nullradix_app::dispatch::start::TicketStarter;
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
use nullradix_app::runs::RunService;
use nullradix_app::sessions::SessionService;
use nullradix_app::themes::ThemeService;
use nullradix_app::tickets::TicketService;
use nullradix_app::usage::UsageService;
use nullradix_app::worktrees::WorktreeService;
use nullradix_app::{ChangeSignal, Clock, SystemClock, UuidV7Generator};
use nullradix_client::{Discovery, DiscoveryFile, Spool};
use nullradix_domain::{Actor, ConfigDir, Timestamp};
use nullradix_runner::{
    CurlTokenEndpoint, CurlUsageApi, LocalAccountFolders, LocalClaudeLogins, LocalLoginRenewal,
    LocalLoginRunner, LocalRunner, LocalSetup, SupervisorCommand, TranscriptRoots,
};
use nullradix_store::Store;
use tokio::net::TcpListener;
use tokio::task;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::intake;
use crate::logging;
use crate::secrets::OsSecretStore;
use crate::themes::FolderThemes;
use crate::token;

/// Time open connections get to finish once a shutdown is requested.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
/// How often the daemon checks on active runs.
const RUN_POLL: Duration = Duration::from_millis(500);

pub(crate) fn run(config: &Config) -> anyhow::Result<()> {
    let _logging = logging::init(&config.paths.logs, &config.log_filter)?;
    let data = &config.paths.data;
    fs::create_dir_all(data).with_context(|| format!("cannot create {}", data.display()))?;
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
    let store = Store::open(&config.paths.data.join("nullradix.db"))
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
    let (themes, customizations) = start_extensions(config, &store, &signal);
    let (favorites, sessions) = start_sessions(&store, &runners, &signal);
    let hooks = start_hooks(config, &store, &sessions, &shutdown);
    let dispatcher_prompts = dispatcher_prompt_service(&dispatcher_config, &board);
    let (dispatcher, dispatching) = start_dispatcher(
        &store,
        &signal,
        &shutdown,
        DispatcherParts {
            config: dispatcher_config.clone(),
            runners: runners.clone(),
            runs: runs.clone(),
            dispatch: dispatch_runs.clone(),
            prompts: dispatcher_prompts.clone(),
            board: &board,
        },
    );
    let state = ApiState {
        auth,
        projects,
        accounts: account_services.accounts,
        dispatcher_config,
        dispatcher_prompts,
        dispatcher: dispatcher.clone(),
        usage: account_services.usage,
        logins: account_services.logins.clone(),
        boards: board.boards,
        tickets: board.tickets,
        comments: board.comments,
        questions: board.questions,
        worktrees: board.worktrees,
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
        customizations,
        changes: Arc::new(store.clone()),
        signal,
        shutdown: shutdown.clone(),
        allowed_hosts: AllowedHosts::loopback_and(config.allowed_hosts.iter().cloned()),
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

/// Local time as the machine tells it, read afresh because daylight saving moves it.
struct OsLocalTime;

#[async_trait::async_trait]
impl LocalTime for OsLocalTime {
    async fn minute_of_day(&self, at: Timestamp) -> u32 {
        let offset = task::spawn_blocking(nullradix_os::local_utc_offset_minutes)
            .await
            .ok()
            .flatten();
        if offset.is_none() {
            tracing::warn!("cannot read the local time offset, the time window uses UTC");
        }
        minute_with_offset(at, offset.unwrap_or(0))
    }
}

/// What the dispatcher needs from the other services.
struct DispatcherParts<'a> {
    config: Arc<DispatcherConfigService>,
    runners: Arc<RunnerService>,
    runs: Arc<RunService>,
    dispatch: Arc<DispatchRuns>,
    prompts: Arc<DispatcherPromptService>,
    board: &'a BoardServices,
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

/// The dispatcher and its beat: a pass every minute until the daemon stops.
fn start_dispatcher(
    store: &Store,
    signal: &ChangeSignal,
    shutdown: &CancellationToken,
    parts: DispatcherParts<'_>,
) -> (Arc<DispatcherService>, Dispatching) {
    let dispatcher = assemble(DispatcherDeps {
        config: parts.config,
        states: Arc::new(store.clone()),
        log: Arc::new(store.clone()),
        setup: Arc::new(LocalSetup),
        actors: Arc::new(store.clone()),
        boards: Arc::new(store.clone()),
        runners: parts.runners,
        runs: parts.runs,
        dispatch: parts.dispatch,
        prompts: parts.prompts,
        tickets: parts.board.tickets.clone(),
        comments: parts.board.comments.clone(),
        worktrees: parts.board.worktrees.clone(),
        clock: Arc::new(SystemClock),
        ids: Arc::new(UuidV7Generator),
        signal: signal.clone(),
        local_time: Arc::new(OsLocalTime),
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
    (dispatcher.service, dispatching)
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
    } else {
        LocalLoginRunner::claude()
    }
}

/// New accounts get folders in the data folder; a home override also moves `~` there.
fn account_folders(config: &Config) -> LocalAccountFolders {
    let folders = LocalAccountFolders::new(config.paths.data.join("accounts"));
    if config.isolated {
        folders.with_home(Some(config.paths.data.clone()))
    } else {
        folders
    }
}

/// Claude's login files: the real ones, or none of them when a home override isolates the run.
fn claude_logins(config: &Config) -> LocalClaudeLogins {
    if config.isolated {
        LocalClaudeLogins::new(Some(config.paths.data.clone()), None)
    } else {
        LocalClaudeLogins::from_environment()
    }
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

/// Themes from the data folder and the customizations in the database.
fn start_extensions(
    config: &Config,
    store: &Store,
    signal: &ChangeSignal,
) -> (Arc<ThemeService>, Arc<CustomizationService>) {
    let themes = Arc::new(ThemeService::new(
        Arc::new(store.clone()),
        Arc::new(FolderThemes::new(config.paths.themes())),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let customizations = Arc::new(CustomizationService::new(
        Arc::new(store.clone()),
        Arc::new(NoStandards),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    (themes, customizations)
}

struct BoardServices {
    boards: Arc<BoardService>,
    tickets: Arc<TicketService>,
    comments: Arc<CommentService>,
    questions: Arc<QuestionService>,
    worktrees: Arc<WorktreeService>,
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
        LocalRunner::new(config.paths.data.join("runs"), supervisor)
            .with_live_transcript_roots(roots),
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
    use std::path::PathBuf;
    use std::time::Duration;

    use nullradix_app::secrets::MemorySecretStore;

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

        let max = accounts.create("Max", None).await.unwrap();
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
}
