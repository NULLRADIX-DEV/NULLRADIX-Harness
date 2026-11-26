//! Puts the dispatcher's parts together, so every composition root wires them alike.

use std::sync::Arc;
use std::time::Duration;

use super::accounts::{
    AccountLimits, AccountPauseRepository, AccountPauses, AccountPicker, PickerParts,
    UsageSampleRepository,
};
use super::candidates::Candidates;
use super::control::{ControlParts, DispatcherService};
use super::evaluate::EvaluateRuns;
use super::local_time::LocalTime;
use super::log::{DispatchLog, DispatcherLogRepository};
use super::pass::{DispatchPass, PassPhase};
use super::runs::DispatchRuns;
use super::setup::WorktreeSetupPort;
use super::stale::ReleaseStale;
use super::start::{StarterParts, TicketStarter};
use super::start_ready::StartReady;
use super::state::{DispatcherStateRepository, DispatcherStateService};
use crate::accounts::AccountService;
use crate::boards::{ActorRepository, BoardRepository};
use crate::comments::CommentService;
use crate::dispatcher_config::DispatcherConfigService;
use crate::dispatcher_prompts::DispatcherPromptService;
use crate::repositories::RunnerService;
use crate::runs::RunService;
use crate::tickets::TicketService;
use crate::worktrees::WorktreeService;
use crate::{ChangeSignal, Clock, IdGenerator};

/// What the dispatcher is built from.
pub struct DispatcherDeps {
    pub config: Arc<DispatcherConfigService>,
    pub states: Arc<dyn DispatcherStateRepository>,
    pub log: Arc<dyn DispatcherLogRepository>,
    pub accounts: Arc<AccountService>,
    pub limits: Arc<dyn AccountLimits>,
    pub pauses: Arc<dyn AccountPauseRepository>,
    pub samples: Arc<dyn UsageSampleRepository>,
    pub setup: Arc<dyn WorktreeSetupPort>,
    pub actors: Arc<dyn ActorRepository>,
    pub boards: Arc<dyn BoardRepository>,
    pub runners: Arc<RunnerService>,
    pub runs: Arc<RunService>,
    pub dispatch: Arc<DispatchRuns>,
    pub prompts: Arc<DispatcherPromptService>,
    pub tickets: Arc<TicketService>,
    pub comments: Arc<CommentService>,
    pub worktrees: Arc<WorktreeService>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub signal: ChangeSignal,
    pub local_time: Arc<dyn LocalTime>,
    /// Time between scheduled passes; none when no scheduler runs.
    pub interval: Option<Duration>,
}

pub struct Dispatcher {
    pub service: Arc<DispatcherService>,
    pub pass: Arc<DispatchPass>,
    pub starter: Arc<TicketStarter>,
    pub accounts: Arc<AccountPicker>,
    pub pauses: Arc<AccountPauses>,
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher").finish_non_exhaustive()
    }
}

/// The pass with its steps of this slice and the service that controls it.
#[allow(
    clippy::too_many_lines,
    reason = "the wiring of every step of the pass reads best in one place"
)]
#[must_use]
pub fn assemble(deps: DispatcherDeps) -> Dispatcher {
    let log = Arc::new(DispatchLog::new(
        deps.log,
        deps.clock.clone(),
        deps.ids.clone(),
    ));
    let state = Arc::new(DispatcherStateService::new(
        deps.states,
        deps.clock.clone(),
        deps.signal.clone(),
    ));
    let pauses = Arc::new(AccountPauses::new(
        deps.pauses,
        deps.accounts.clone(),
        state.clone(),
        deps.clock.clone(),
        deps.signal,
    ));
    let accounts = Arc::new(AccountPicker::new(PickerParts {
        accounts: deps.accounts,
        limits: deps.limits,
        pauses: pauses.clone(),
        samples: deps.samples,
        runs: deps.runs.clone(),
        clock: deps.clock.clone(),
        ids: deps.ids.clone(),
    }));
    let candidates = Arc::new(Candidates::new(
        deps.tickets.clone(),
        deps.boards.clone(),
        deps.runners.clone(),
    ));
    let starter = TicketStarter::new(StarterParts {
        candidates: candidates.clone(),
        tickets: deps.tickets.clone(),
        comments: deps.comments.clone(),
        worktrees: deps.worktrees,
        runners: deps.runners,
        runs: deps.runs.clone(),
        dispatch: deps.dispatch,
        accounts: accounts.clone(),
        prompts: deps.prompts,
        setup: deps.setup,
        actors: deps.actors,
        log: log.clone(),
        clock: deps.clock.clone(),
        ids: deps.ids,
    });
    let pass = DispatchPass::new(
        deps.config.clone(),
        state.clone(),
        deps.clock.clone(),
        deps.local_time.clone(),
        log.clone(),
    );
    pass.set_step(
        PassPhase::Evaluate,
        Arc::new(
            EvaluateRuns::new(
                deps.tickets.clone(),
                deps.comments.clone(),
                deps.runs.clone(),
                starter.clone(),
                log.clone(),
            )
            .with_accounts(accounts.clone()),
        ),
    );
    pass.set_step(
        PassPhase::ReleaseStale,
        Arc::new(ReleaseStale::new(
            deps.tickets,
            deps.comments,
            deps.runs.clone(),
            starter.clone(),
        )),
    );
    pass.set_step(
        PassPhase::StartReady,
        Arc::new(StartReady::new(
            candidates.clone(),
            starter.clone(),
            deps.runs.clone(),
        )),
    );
    let service = Arc::new(DispatcherService::new(ControlParts {
        config: deps.config,
        state,
        pass: pass.clone(),
        candidates,
        starter: starter.clone(),
        accounts: accounts.clone(),
        runs: deps.runs,
        boards: deps.boards,
        clock: deps.clock,
        local: deps.local_time,
        interval: deps.interval,
    }));
    Dispatcher {
        service,
        pass,
        starter,
        accounts,
        pauses,
    }
}
