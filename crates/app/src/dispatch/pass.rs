//! One dispatcher pass: its steps in a fixed order, one pass at a time.

use std::future::Future;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use nullradix_domain::{DispatcherConfig, IdleReason, Run, RunKind, Timestamp, idle_reason};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex as AsyncMutex, watch};

use super::local_time::LocalTime;
use super::log::DispatchLog;
use super::state::DispatcherStateService;
use crate::dispatcher_config::DispatcherConfigService;
use crate::{AppError, Clock};

/// The places of a pass, in the order they run. Later slices fill the free ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PassPhase {
    /// Settle finished runs: outcomes, retries, escalation, pauses (evaluation of runs).
    Evaluate,
    /// Remove research worktrees whose runs are gone (research).
    ResearchOrphans,
    /// Give back claims whose agent shows no sign of life.
    ReleaseStale,
    /// Queue merges for done tickets that have none (merge catch-up).
    MergeCatchUp,
    /// Process the merge jobs of every branch (merging).
    MergeAll,
    /// Start ready tickets up to the limit; planning follows here later.
    StartReady,
    /// Start research on open questions: autopilot ones first, then the automation.
    StartResearch,
    /// Write down what evaluated or failed runs left for the dashboard (run recorder).
    RecorderFlush,
}

impl PassPhase {
    pub const ALL: [Self; 8] = [
        Self::Evaluate,
        Self::ResearchOrphans,
        Self::ReleaseStale,
        Self::MergeCatchUp,
        Self::MergeAll,
        Self::StartReady,
        Self::StartResearch,
        Self::RecorderFlush,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Evaluate => "evaluate",
            Self::ResearchOrphans => "research-orphans",
            Self::ReleaseStale => "release-stale",
            Self::MergeCatchUp => "merge-catch-up",
            Self::MergeAll => "merge-all",
            Self::StartReady => "start-ready",
            Self::StartResearch => "start-research",
            Self::RecorderFlush => "recorder-flush",
        }
    }

    const fn slot(self) -> usize {
        self as usize
    }
}

/// What a pass did, kept for the status.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassReport {
    /// Unix milliseconds the pass began.
    pub at: i64,
    /// Why nothing new started, in words.
    pub idle: Option<String>,
    /// Why a ready ticket did not start for want of a Claude account.
    #[serde(default)]
    pub waiting: Option<String>,
    pub started: Vec<String>,
    /// Questions whose research started.
    #[serde(default)]
    pub research: Vec<String>,
    /// Tickets whose branch went into its target.
    #[serde(default)]
    pub merged: Vec<String>,
    /// Ready tickets that wait for a free place.
    pub queued: Vec<String>,
    pub released: Vec<String>,
    pub errors: Vec<String>,
}

impl PassReport {
    #[must_use]
    pub fn new(at: Timestamp) -> Self {
        Self {
            at: at.as_millis(),
            ..Self::default()
        }
    }

    /// A pass that did something or failed is kept in the database; an idle one is not.
    #[must_use]
    pub fn worth_keeping(&self) -> bool {
        !(self.started.is_empty()
            && self.research.is_empty()
            && self.merged.is_empty()
            && self.released.is_empty()
            && self.errors.is_empty())
    }

    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    #[must_use]
    pub fn from_json(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }
}

/// What a step knows of the pass it runs in.
#[derive(Debug, Clone)]
pub struct PassContext {
    pub config: DispatcherConfig,
    pub now: Timestamp,
    /// Why no new ticket may start, if so; steps that only start things stand down.
    pub idle: Option<IdleReason>,
}

#[async_trait]
pub trait PassStep: Send + Sync {
    /// A failure goes into the report and the next step runs anyway.
    async fn run(&self, context: &PassContext, report: &mut PassReport) -> Result<(), AppError>;
}

/// Runs that count for the limit of tickets worked on at the same time.
#[must_use]
pub fn counts_for_wip(run: &Run) -> bool {
    run.dispatch()
        .is_some_and(|dispatch| matches!(dispatch.kind, RunKind::Work | RunKind::Merge))
}

/// The words for an idle reason.
#[must_use]
pub fn idle_message(reason: &IdleReason) -> String {
    match reason {
        IdleReason::Disabled => String::from("The dispatcher is off"),
        IdleReason::Paused(_) => String::from("Paused"),
        IdleReason::OutsideWindow { from, to } => {
            format!("Outside the time window {from}-{to}")
        }
    }
}

type Slots = [Option<Arc<dyn PassStep>>; PassPhase::ALL.len()];

pub struct DispatchPass {
    config: Arc<DispatcherConfigService>,
    state: Arc<DispatcherStateService>,
    clock: Arc<dyn Clock>,
    local: Arc<dyn LocalTime>,
    log: Arc<DispatchLog>,
    steps: Mutex<Slots>,
    /// Shared by everything that must not run beside a pass: merges, accepts, returns.
    lock: AsyncMutex<()>,
    /// The pass that runs now; never held across an await.
    flight: Mutex<Option<watch::Receiver<Option<PassReport>>>>,
    last: Mutex<Option<PassReport>>,
    me: Weak<Self>,
}

/// Frees the place of the running pass, also when its task stops early.
struct FlightGuard<'a>(&'a DispatchPass);

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        *self.0.flight.lock() = None;
    }
}

impl std::fmt::Debug for DispatchPass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchPass").finish_non_exhaustive()
    }
}

impl DispatchPass {
    #[must_use]
    pub fn new(
        config: Arc<DispatcherConfigService>,
        state: Arc<DispatcherStateService>,
        clock: Arc<dyn Clock>,
        local: Arc<dyn LocalTime>,
        log: Arc<DispatchLog>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            config,
            state,
            clock,
            local,
            log,
            steps: Mutex::new(std::array::from_fn(|_| None)),
            lock: AsyncMutex::new(()),
            flight: Mutex::new(None),
            last: Mutex::new(None),
            me: me.clone(),
        })
    }

    /// Puts a step in its place; a step there before is replaced.
    pub fn set_step(&self, phase: PassPhase, step: Arc<dyn PassStep>) {
        self.steps.lock()[phase.slot()] = Some(step);
    }

    /// The report of the last pass of this process.
    #[must_use]
    pub fn last_report(&self) -> Option<PassReport> {
        self.last.lock().clone()
    }

    /// Runs `work` once every earlier pass and exclusive work is done, and none starts beside it.
    pub async fn exclusive<T, F, Fut>(&self, work: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let _guard = self.lock.lock().await;
        work().await
    }

    /// One pass on its own task; a caller who comes while one runs gets its result.
    pub async fn run(&self) -> PassReport {
        let Some(this) = self.me.upgrade() else {
            return self.exclusive(|| self.pass()).await;
        };
        // a pass that ended without a result (it panicked) is tried once more
        for _ in 0..2 {
            let mut receiver = Self::join_or_start(&this);
            loop {
                if let Some(report) = receiver.borrow_and_update().clone() {
                    return report;
                }
                if receiver.changed().await.is_err() {
                    break;
                }
            }
        }
        let mut report = PassReport::new(self.clock.now());
        report
            .errors
            .push(String::from("the dispatcher pass stopped without a result"));
        report
    }

    /// Waits until the pass under way, if any, has ended.
    pub async fn settle(&self) {
        loop {
            let Some(mut receiver) = self.flight.lock().clone() else {
                return;
            };
            while receiver.borrow_and_update().is_none() && receiver.changed().await.is_ok() {}
        }
    }

    fn join_or_start(this: &Arc<Self>) -> watch::Receiver<Option<PassReport>> {
        let mut flight = this.flight.lock();
        if let Some(receiver) = flight.clone() {
            return receiver;
        }
        let (sender, receiver) = watch::channel(None);
        *flight = Some(receiver.clone());
        let this = Arc::clone(this);
        tokio::spawn(async move {
            let guard = FlightGuard(&this);
            let report = this.exclusive(|| this.pass()).await;
            drop(guard);
            // nobody may be waiting
            let _ = sender.send(Some(report));
        });
        receiver
    }

    async fn pass(&self) -> PassReport {
        let now = self.clock.now();
        let mut report = PassReport::new(now);
        self.log.prune().await;
        if let Err(error) = self.steps(now, &mut report).await {
            report.errors.push(error.message().to_owned());
        }
        for error in &report.errors {
            self.log.error(error, None).await;
        }
        *self.last.lock() = Some(report.clone());
        if report.worth_keeping()
            && let Err(error) = self.state.record_pass(&report).await
        {
            tracing::warn!(%error, "cannot record the dispatcher pass");
        }
        report
    }

    async fn idle_now(
        &self,
        config: &DispatcherConfig,
        now: Timestamp,
    ) -> Result<Option<IdleReason>, AppError> {
        let state = self.state.get().await?;
        let minute = self.local.minute_of_day(now).await;
        Ok(idle_reason(
            state.enabled(config.enabled),
            state.paused_until(now),
            config.window.as_ref(),
            now,
            minute,
        ))
    }

    async fn steps(&self, now: Timestamp, report: &mut PassReport) -> Result<(), AppError> {
        let config = self.config.get().await?.config;
        let idle = self.idle_now(&config, now).await?;
        if let Some(reason) = &idle {
            report.idle = Some(idle_message(reason));
        }
        let mut context = PassContext { config, now, idle };
        let steps: Vec<(PassPhase, Arc<dyn PassStep>)> = {
            let slots = self.steps.lock();
            PassPhase::ALL
                .into_iter()
                .filter_map(|phase| slots[phase.slot()].clone().map(|step| (phase, step)))
                .collect()
        };
        for (phase, step) in steps {
            if let Err(error) = step.run(&context, report).await {
                report
                    .errors
                    .push(format!("{}: {}", phase.name(), error.message()));
            }
            // settling runs may pause the dispatcher; nothing new starts after that
            if phase == PassPhase::Evaluate && context.idle.is_none() {
                match self.idle_now(&context.config, now).await {
                    Ok(idle) => context.idle = idle,
                    Err(error) => {
                        report
                            .errors
                            .push(format!("{}: {}", phase.name(), error.message()));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phases_keep_the_legacy_order() {
        let mut sorted = PassPhase::ALL;
        sorted.sort();
        assert_eq!(sorted, PassPhase::ALL);
        assert_eq!(PassPhase::ReleaseStale.name(), "release-stale");
        assert_eq!(PassPhase::StartReady.slot(), 5);
    }

    #[test]
    fn a_report_round_trips_and_idle_ones_are_not_kept() {
        let mut report = PassReport::new(Timestamp::from_millis(7));
        assert!(!report.worth_keeping());
        report.started.push(String::from("ARC-0001"));
        assert!(report.worth_keeping());
        assert_eq!(PassReport::from_json(&report.to_json()), Some(report));
        assert_eq!(PassReport::from_json("nope"), None);
    }
}
