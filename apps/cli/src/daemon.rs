use std::fs::{self, File, TryLockError};
use std::path::Path;

use anyhow::{Context, bail};
use nullradix_api::API_VERSION;
use nullradix_app::{Clock, SystemClock};
use nullradix_client::{Discovery, DiscoveryFile};
use nullradix_store::Store;
use tokio::net::TcpListener;

use crate::config::Config;
use crate::logging;

pub(crate) fn run(config: &Config) -> anyhow::Result<()> {
    let _logging = logging::init(&config.paths.logs, &config.log_filter)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("nullradix")
        .build()
        .context("cannot start the async runtime")?;
    runtime.block_on(serve(config))
}

async fn serve(config: &Config) -> anyhow::Result<()> {
    let data = &config.paths.data;
    fs::create_dir_all(data).with_context(|| format!("cannot create {}", data.display()))?;
    let _lock = lock_profile(data)?;
    let store = Store::open(&data.join("nullradix.db"))
        .await
        .context("cannot open the database")?;
    let listener = TcpListener::bind(config.address)
        .await
        .with_context(|| format!("cannot listen on {}", config.address))?;
    let address = listener.local_addr()?;
    let discovery = DiscoveryFile::in_dir(data);
    discovery.write(&Discovery {
        pid: std::process::id(),
        address,
        version: String::from(env!("CARGO_PKG_VERSION")),
        api_version: API_VERSION,
        started_at: SystemClock.now().as_millis(),
    })?;
    tracing::info!(
        %address,
        profile = ?config.profile,
        data = %data.display(),
        "daemon started"
    );
    if !address.ip().is_loopback() {
        tracing::warn!(%address, "listening beyond loopback without authentication");
    }
    let served = nullradix_api::serve(listener, nullradix_api::router(), shutdown_signal())
        .await
        .context("the HTTP server failed");
    discovery.remove()?;
    store.close().await;
    served?;
    tracing::info!("daemon stopped");
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

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
    tracing::info!("shutdown requested");
}
