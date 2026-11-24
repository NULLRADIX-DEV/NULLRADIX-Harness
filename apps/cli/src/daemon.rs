use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use nullradix_api::{API_VERSION, AllowedHosts, ApiState};
use nullradix_app::auth::AuthService;
use nullradix_app::hooks::HookService;
use nullradix_app::projects::ProjectService;
use nullradix_app::runs::RunService;
use nullradix_app::{ChangeSignal, Clock, ErrorKind, SystemClock, UuidV7Generator};
use nullradix_client::{Discovery, DiscoveryFile, Spool, SpooledHook};
use nullradix_runner::{LocalRunner, SupervisorCommand};
use nullradix_store::Store;
use tokio::net::TcpListener;
use tokio::task;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::logging;
use crate::token;

/// Time open connections get to finish once a shutdown is requested.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
/// How often the daemon checks on active runs.
const RUN_POLL: Duration = Duration::from_millis(500);
/// How often the daemon takes in hooks spooled while it could not answer.
const SPOOL_POLL: Duration = Duration::from_secs(10);
const SPOOL_BATCH: usize = 500;
/// Hook events older than this are forgotten.
const HOOK_RETENTION: Duration = Duration::from_hours(24 * 30);

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
    let signal = ChangeSignal::new();
    let shutdown = CancellationToken::new();
    let projects = Arc::new(ProjectService::new(
        Arc::new(store.clone()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let runs = start_runs(config, &store, &signal).await?;
    let hooks = start_hooks(config, &store, &shutdown).await?;
    let state = ApiState {
        auth,
        projects,
        runs,
        hooks,
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

/// Run service on the local runner, watching runs that outlived the previous daemon.
async fn start_runs(
    config: &Config,
    store: &Store,
    signal: &ChangeSignal,
) -> anyhow::Result<Arc<RunService>> {
    let supervisor = SupervisorCommand {
        program: std::env::current_exe().context("cannot locate the nullradix binary")?,
        args: vec![String::from("run-supervisor")],
    };
    let runs = RunService::new(
        Arc::new(store.clone()),
        Arc::new(LocalRunner::new(config.paths.data.join("runs"), supervisor)),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
        RUN_POLL,
    );
    let recovered = runs.recover().await.context("cannot recover active runs")?;
    if recovered > 0 {
        tracing::info!(recovered, "watching runs from before the restart");
    }
    Ok(runs)
}

/// Hook service that also takes in what hooks spooled while the daemon was away.
async fn start_hooks(
    config: &Config,
    store: &Store,
    shutdown: &CancellationToken,
) -> anyhow::Result<Arc<HookService>> {
    let hooks = Arc::new(HookService::new(
        Arc::new(store.clone()),
        Arc::new(SystemClock),
    ));
    let pruned = hooks.prune(HOOK_RETENTION).await?;
    tracing::debug!(pruned, "old hook events removed");
    let spool = Spool::in_dir(&config.paths.data);
    let service = Arc::clone(&hooks);
    let stop = shutdown.clone();
    tokio::spawn(async move {
        loop {
            match drain_spool(spool.clone(), &service).await {
                Ok(0) => {}
                Ok(taken) => tracing::info!(taken, "spooled hooks taken in"),
                Err(error) => tracing::warn!(error = %error, "cannot take in spooled hooks"),
            }
            tokio::select! {
                () = stop.cancelled() => break,
                () = tokio::time::sleep(SPOOL_POLL) => {}
            }
        }
    });
    Ok(hooks)
}

/// Takes in a batch of spooled hooks, oldest first; stops when the store fails.
async fn drain_spool(spool: Spool, hooks: &HookService) -> anyhow::Result<usize> {
    let batch = task::spawn_blocking(
        move || -> io::Result<Vec<(PathBuf, io::Result<SpooledHook>)>> {
            let pending = spool.pending()?.into_iter().take(SPOOL_BATCH);
            Ok(pending
                .map(|path| {
                    let hook = Spool::read(&path);
                    (path, hook)
                })
                .collect())
        },
    )
    .await??;
    let mut taken = Vec::new();
    let mut rejected = Vec::new();
    let mut failure = None;
    for (path, hook) in batch {
        let hook = match hook {
            Ok(hook) => hook,
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                tracing::warn!(path = %path.display(), %error, "unreadable spooled hook set aside");
                rejected.push(path);
                continue;
            }
            // gone already or locked by a scanner; the next round sees it again
            Err(_) => continue,
        };
        let received = match hook.event.into_event(hook.kind) {
            Ok(event) => hooks.receive(&event).await,
            Err(error) => Err(error),
        };
        match received {
            Ok(_) => taken.push(path),
            Err(error) if error.kind() == ErrorKind::Invalid => {
                tracing::warn!(path = %path.display(), %error, "invalid spooled hook set aside");
                rejected.push(path);
            }
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    let count = taken.len();
    task::spawn_blocking(move || -> io::Result<()> {
        for path in &taken {
            Spool::remove(path)?;
        }
        for path in &rejected {
            Spool::set_aside(path)?;
        }
        Ok(())
    })
    .await??;
    match failure {
        Some(error) => Err(error.into()),
        None => Ok(count),
    }
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
