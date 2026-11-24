use anyhow::Context;
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
    let listener = TcpListener::bind(config.address)
        .await
        .with_context(|| format!("cannot listen on {}", config.address))?;
    tracing::info!(
        address = %listener.local_addr()?,
        profile = ?config.profile,
        data = %config.paths.data.display(),
        "daemon started"
    );
    nullradix_api::serve(listener, nullradix_api::router(), shutdown_signal())
        .await
        .context("the HTTP server failed")?;
    tracing::info!("daemon stopped");
    Ok(())
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
