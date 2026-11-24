use std::fs;
use std::path::Path;

use anyhow::Context;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

const KEPT_LOG_FILES: usize = 14;

/// Logs compact lines to stderr and JSON to a daily rolling file; keep the guard alive.
pub(crate) fn init(logs: &Path, filter: &str) -> anyhow::Result<WorkerGuard> {
    fs::create_dir_all(logs).with_context(|| format!("cannot create {}", logs.display()))?;
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("nullradix")
        .filename_suffix("log")
        .max_log_files(KEPT_LOG_FILES)
        .build(logs)
        .context("cannot open the log file")?;
    let (file, guard) = tracing_appender::non_blocking(appender);
    let filter =
        EnvFilter::try_new(filter).with_context(|| format!("invalid log filter `{filter}`"))?;
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().compact().with_writer(std::io::stderr))
        .with(fmt::layer().json().with_writer(file))
        .try_init()
        .context("logging is already initialised")?;
    Ok(guard)
}
