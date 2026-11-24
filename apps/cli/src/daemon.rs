use std::fs::{self, File, TryLockError};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use nullradix_api::{API_VERSION, AllowedHosts, ApiState};
use nullradix_app::auth::AuthService;
use nullradix_app::{Clock, SystemClock, UuidV7Generator};
use nullradix_client::{Discovery, DiscoveryFile};
use nullradix_store::Store;
use tokio::net::TcpListener;
use tokio::task;

use crate::config::Config;
use crate::logging;
use crate::token;

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
    let listener = TcpListener::bind(config.address)
        .await
        .with_context(|| format!("cannot listen on {}", config.address))?;
    let address = listener.local_addr()?;
    let record = Discovery {
        pid: std::process::id(),
        address: advertised(address),
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
    if !address.ip().is_loopback() {
        tracing::warn!(%address, "listening beyond loopback without authentication");
    }
    let state = ApiState {
        auth,
        allowed_hosts: AllowedHosts::loopback_and(config.allowed_hosts.iter().cloned()),
        secure_cookies: false,
    };
    let served = nullradix_api::serve(listener, nullradix_api::router(state), shutdown_signal())
        .await
        .context("the HTTP server failed");
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

/// Address clients should dial; a wildcard bind is reachable through loopback.
fn advertised(address: SocketAddr) -> SocketAddr {
    let ip = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, address.port())
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
    use super::*;

    #[test]
    fn advertises_loopback_for_wildcard_binds() {
        let v4: SocketAddr = "0.0.0.0:4711".parse().unwrap();
        let v6: SocketAddr = "[::]:4711".parse().unwrap();
        let fixed: SocketAddr = "192.168.1.5:4711".parse().unwrap();
        assert_eq!(advertised(v4), "127.0.0.1:4711".parse().unwrap());
        assert_eq!(advertised(v6), "[::1]:4711".parse().unwrap());
        assert_eq!(advertised(fixed), fixed);
    }
}
