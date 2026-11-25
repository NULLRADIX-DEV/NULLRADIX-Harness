//! Keeps slow git work in a checkout from piling up when requests come faster than it ends.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// One request at a time per checkout; later ones wait for their turn.
#[derive(Debug, Default)]
pub(crate) struct CheckoutGate {
    slots: Mutex<HashMap<PathBuf, Arc<Semaphore>>>,
}

impl CheckoutGate {
    /// Waits until no other request works in `checkout`; the permit frees the turn on drop.
    pub(crate) async fn enter(&self, checkout: &Path) -> Option<OwnedSemaphorePermit> {
        let slot = {
            let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
            // checkouts nobody waits for or works in drop out
            slots.retain(|_, slot| Arc::strong_count(slot) > 1);
            Arc::clone(
                slots
                    .entry(checkout.to_path_buf())
                    .or_insert_with(|| Arc::new(Semaphore::new(1))),
            )
        };
        // the semaphore is never closed
        slot.acquire_owned().await.ok()
    }
}

/// Tells blocking work its answer is no longer wanted once the guard is dropped.
#[derive(Debug)]
pub(crate) struct Abort(Arc<AtomicBool>);

impl Abort {
    pub(crate) fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub(crate) fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}

impl Drop for Abort {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn a_checkout_serves_one_request_at_a_time() {
        let gate = CheckoutGate::default();
        let first = gate.enter(Path::new("/a")).await.unwrap();
        let other = gate.enter(Path::new("/b")).await;
        let waiting = tokio::time::timeout(Duration::from_millis(50), gate.enter(Path::new("/a")));
        assert!(waiting.await.is_err(), "the second request waits");
        drop(first);
        let second = tokio::time::timeout(Duration::from_secs(5), gate.enter(Path::new("/a")))
            .await
            .unwrap();
        drop((second, other));
        gate.enter(Path::new("/c")).await;
        assert_eq!(
            gate.slots.lock().unwrap().len(),
            1,
            "idle checkouts drop out"
        );
    }

    #[test]
    fn dropping_the_guard_aborts_the_work() {
        let abort = Abort::new();
        let flag = abort.flag();
        assert!(!flag.load(Ordering::Relaxed));
        drop(abort);
        assert!(flag.load(Ordering::Relaxed));
    }
}
