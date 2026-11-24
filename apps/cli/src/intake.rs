//! Takes in hooks that were spooled while the daemon could not answer.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nullradix_app::ErrorKind;
use nullradix_app::hooks::HookService;
use nullradix_client::{Spool, SpooledHook};
use tokio::task;
use tokio_util::sync::CancellationToken;

const POLL: Duration = Duration::from_secs(10);
const BATCH: usize = 500;
/// Hook events received longer ago are forgotten.
const RETENTION: Duration = Duration::from_hours(24 * 30);
const PRUNE_EVERY: Duration = Duration::from_hours(24);
/// Staging files this old belong to hooks that were stopped while spooling.
const STALE_STAGING: Duration = Duration::from_secs(60);

enum Verdict {
    Taken,
    Rejected,
    Postponed,
}

/// Runs until `shutdown`: takes in the spool every few seconds and prunes old events daily.
pub(crate) fn start(hooks: Arc<HookService>, spool: Spool, shutdown: CancellationToken) {
    tokio::spawn(async move {
        let retry = spool.clone();
        match task::spawn_blocking(move || retry.retry_postponed()).await {
            Ok(Ok(0)) => {}
            Ok(Ok(retried)) => tracing::info!(retried, "postponed spooled hooks retried"),
            Ok(Err(error)) => tracing::warn!(%error, "cannot retry postponed spooled hooks"),
            Err(error) => tracing::warn!(%error, "cannot retry postponed spooled hooks"),
        }
        let mut pruned_at: Option<Instant> = None;
        loop {
            if pruned_at.is_none_or(|at| at.elapsed() >= PRUNE_EVERY) {
                match hooks.prune(RETENTION).await {
                    Ok(pruned) => tracing::debug!(pruned, "old hook events removed"),
                    Err(error) => tracing::warn!(%error, "cannot remove old hook events"),
                }
                pruned_at = Some(Instant::now());
            }
            match drain(spool.clone(), &hooks).await {
                Ok(0) => {}
                Ok(taken) => tracing::info!(taken, "spooled hooks taken in"),
                Err(error) => tracing::warn!(error = %error, "cannot take in spooled hooks"),
            }
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(POLL) => {}
            }
        }
    });
}

/// Takes in one batch, oldest first; stops at the first failure of the store.
async fn drain(spool: Spool, hooks: &HookService) -> anyhow::Result<usize> {
    let batch = task::spawn_blocking(move || read_batch(&spool)).await??;
    let mut verdicts = Vec::new();
    let mut failure = None;
    for (path, hook) in batch {
        let hook = match hook {
            Ok(hook) => hook,
            Err(error) => {
                match error.kind() {
                    io::ErrorKind::InvalidData => {
                        tracing::warn!(path = %path.display(), %error, "unreadable spooled hook set aside");
                        verdicts.push((path, Verdict::Rejected));
                    }
                    io::ErrorKind::Unsupported => {
                        tracing::warn!(path = %path.display(), %error, "spooled hook of a newer version postponed");
                        verdicts.push((path, Verdict::Postponed));
                    }
                    io::ErrorKind::NotFound => {}
                    _ => tracing::warn!(path = %path.display(), %error, "cannot read spooled hook"),
                }
                continue;
            }
        };
        let received = match hook.event.into_event(hook.kind) {
            Ok(event) => hooks.receive(&event).await,
            Err(error) => Err(error),
        };
        match received {
            Ok(_) => verdicts.push((path, Verdict::Taken)),
            Err(error) if error.kind() == ErrorKind::Invalid => {
                tracing::warn!(path = %path.display(), %error, "invalid spooled hook set aside");
                verdicts.push((path, Verdict::Rejected));
            }
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    let taken = verdicts
        .iter()
        .filter(|(_, verdict)| matches!(verdict, Verdict::Taken))
        .count();
    task::spawn_blocking(move || settle(&verdicts)).await?;
    match failure {
        Some(error) => Err(error.into()),
        None => Ok(taken),
    }
}

type Batch = Vec<(PathBuf, io::Result<SpooledHook>)>;

fn read_batch(spool: &Spool) -> io::Result<Batch> {
    if let Err(error) = spool.remove_stale_staging(STALE_STAGING) {
        tracing::warn!(%error, "cannot remove stale spool staging files");
    }
    let pending = spool.pending()?.into_iter().take(BATCH);
    Ok(pending
        .map(|path| {
            let hook = Spool::read(&path);
            (path, hook)
        })
        .collect())
}

/// Tidies each file on its own, so one stuck file cannot hold up the others.
fn settle(verdicts: &[(PathBuf, Verdict)]) {
    for (path, verdict) in verdicts {
        let tidied = match verdict {
            Verdict::Taken => Spool::remove(path),
            Verdict::Rejected => Spool::set_aside(path),
            Verdict::Postponed => Spool::postpone(path),
        };
        if let Err(error) = tidied {
            tracing::warn!(path = %path.display(), %error, "cannot tidy spooled hook");
        }
    }
}
