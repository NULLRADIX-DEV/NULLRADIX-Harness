//! The beat of the dispatcher: one pass a minute, a late beat is delayed and never caught up.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior, interval_at};

use super::pass::DispatchPass;

/// Time between two passes.
pub const PASS_INTERVAL: Duration = Duration::from_mins(1);

/// What the beat triggers; the dispatcher pass in operation, a counter in tests.
#[async_trait]
pub trait Tick: Send + Sync + 'static {
    async fn tick(&self);
}

#[async_trait]
impl Tick for DispatchPass {
    async fn tick(&self) {
        // errors of a pass are in its report and the dispatcher log
        self.run().await;
    }
}

/// Triggers `target` every `period`, the first time one period after the start, until `stop`.
///
/// A pass that takes longer than a period delays the next one instead of making up for it.
pub fn spawn_scheduler(
    target: Arc<dyn Tick>,
    period: Duration,
    stop: impl Future<Output = ()> + Send + 'static,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        tokio::pin!(stop);
        let mut beat = interval_at(Instant::now() + period, period);
        beat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = &mut stop => break,
                _ = beat.tick() => target.tick().await,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::Notify;

    use super::*;

    struct Counter {
        passes: AtomicUsize,
        takes: Duration,
    }

    #[async_trait]
    impl Tick for Counter {
        async fn tick(&self) {
            self.passes.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.takes).await;
        }
    }

    async fn settle() {
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
    }

    fn counter(takes: Duration) -> Arc<Counter> {
        Arc::new(Counter {
            passes: AtomicUsize::new(0),
            takes,
        })
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_pass_comes_after_one_period_and_then_every_period() {
        let target = counter(Duration::ZERO);
        let stop = Arc::new(Notify::new());
        let waiting = Arc::clone(&stop);
        let handle = spawn_scheduler(target.clone(), PASS_INTERVAL, async move {
            waiting.notified().await;
        });
        settle().await;
        assert_eq!(target.passes.load(Ordering::SeqCst), 0);
        tokio::time::advance(Duration::from_secs(61)).await;
        settle().await;
        assert_eq!(target.passes.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(120)).await;
        settle().await;
        assert_eq!(target.passes.load(Ordering::SeqCst), 3);
        stop.notify_one();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_pass_delays_the_next_one_instead_of_catching_up() {
        let target = counter(Duration::from_secs(150));
        let stop = Arc::new(Notify::new());
        let waiting = Arc::clone(&stop);
        let handle = spawn_scheduler(target.clone(), PASS_INTERVAL, async move {
            waiting.notified().await;
        });
        // minutes 1 to 5 would hold five passes; each of these takes two and a half
        for _ in 0..300 {
            tokio::time::advance(Duration::from_secs(1)).await;
            settle().await;
        }
        let passes = target.passes.load(Ordering::SeqCst);
        assert!((2..=3).contains(&passes), "{passes} passes");
        stop.notify_one();
        tokio::time::advance(Duration::from_secs(300)).await;
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn stopping_before_the_first_period_runs_no_pass() {
        let target = counter(Duration::ZERO);
        let handle = spawn_scheduler(target.clone(), PASS_INTERVAL, async {});
        handle.await.unwrap();
        assert_eq!(target.passes.load(Ordering::SeqCst), 0);
    }
}
