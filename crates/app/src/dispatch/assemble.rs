//! Puts the dispatcher's parts together, so every composition root wires them alike.

use std::sync::Arc;
use std::time::Duration;

use super::abort::AbortAttempts;
use super::accounts::{
    AccountLimits, AccountPauseRepository, AccountPauses, AccountPicker, PickerParts,
    UsageSampleRepository,
};
use super::candidates::Candidates;
use super::control::{ControlParts, DispatcherService};
use super::detached::DetachedWorktreePort;
use super::evaluate::EvaluateRuns;
use super::local_time::LocalTime;
use super::log::{DispatchLog, DispatcherLogRepository};
use super::merge::{MergeAll, MergeCatchUp, MergeParts};
use super::merge_git::MergeGit;
use super::merge_jobs::{MergeGate, MergeJobRepository, MergeQueue, QueueOnAccept};
use super::merge_retry::{MergeRetry, RetryParts};
use super::pass::{DispatchPass, PassPhase};
use super::pauses::{DispatchPauses, PauseParts};
use super::planning::{PlanOrphans, PlanningParts, PlanningRuns, StartPlanning};
use super::research::{ResearchOrphans, ResearchParts, ResearchRuns, StartResearch};
use super::review_decide::{DecisionParts, ReviewDecisions};
use super::review_gate::ReviewGate;
use super::runs::DispatchRuns;
use super::setup::WorktreeSetupPort;
use super::stale::ReleaseStale;
use super::start::{StarterParts, TicketStarter};
use super::start_ready::StartReady;
use super::state::{DispatcherStateRepository, DispatcherStateService};
use super::tracking::AutoRunRepository;
use crate::accounts::AccountService;
use crate::boards::{ActorRepository, BoardRepository};
use crate::comments::CommentService;
use crate::dispatcher_config::DispatcherConfigService;
use crate::dispatcher_prompts::DispatcherPromptService;
use crate::notifier::Notifier;
use crate::questions::QuestionService;
use crate::repositories::RunnerService;
use crate::review::ReviewService;
use crate::runs::RunService;
use crate::tickets::TicketService;
use crate::worktrees::{WorktreeRepository, WorktreeService};
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
    pub detached: Arc<dyn DetachedWorktreePort>,
    pub tracking: Arc<dyn AutoRunRepository>,
    pub questions: Arc<QuestionService>,
    pub actors: Arc<dyn ActorRepository>,
    pub boards: Arc<dyn BoardRepository>,
    pub runners: Arc<RunnerService>,
    pub runs: Arc<RunService>,
    pub dispatch: Arc<DispatchRuns>,
    pub prompts: Arc<DispatcherPromptService>,
    pub tickets: Arc<TicketService>,
    pub comments: Arc<CommentService>,
    pub worktrees: Arc<WorktreeService>,
    pub worktree_repository: Arc<dyn WorktreeRepository>,
    pub merge_jobs: Arc<dyn MergeJobRepository>,
    pub merge_git: Arc<dyn MergeGit>,
    pub reviews: Arc<ReviewService>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub signal: ChangeSignal,
    pub local_time: Arc<dyn LocalTime>,
    pub notifier: Arc<dyn Notifier>,
    /// Time between scheduled passes; none when no scheduler runs.
    pub interval: Option<Duration>,
}

pub struct Dispatcher {
    pub service: Arc<DispatcherService>,
    pub pass: Arc<DispatchPass>,
    pub starter: Arc<TicketStarter>,
    pub accounts: Arc<AccountPicker>,
    pub pauses: Arc<AccountPauses>,
    /// Settles finished runs; the pause hook of a later slice is set here.
    pub evaluate: Arc<EvaluateRuns>,
    pub research: Arc<ResearchRuns>,
    pub planning: Arc<PlanningRuns>,
    /// Accept and return of a held review, serialized with the pass.
    pub decisions: Arc<ReviewDecisions>,
    /// Queues merges and lists the jobs of a ticket.
    pub merges: Arc<MergeQueue>,
    /// Retry of a merge, serialized with the pass.
    pub retries: Arc<MergeRetry>,
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
        deps.signal.clone(),
    ));
    let accounts = Arc::new(AccountPicker::new(PickerParts {
        accounts: deps.accounts.clone(),
        limits: deps.limits.clone(),
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
    let aborts = Arc::new(
        AbortAttempts::new(deps.tickets.clone(), deps.comments.clone(), log.clone())
            .with_notifier(deps.notifier.clone()),
    );
    let starter = TicketStarter::new(StarterParts {
        candidates: candidates.clone(),
        tickets: deps.tickets.clone(),
        comments: deps.comments.clone(),
        worktrees: deps.worktrees.clone(),
        runners: deps.runners.clone(),
        runs: deps.runs.clone(),
        dispatch: deps.dispatch.clone(),
        accounts: accounts.clone(),
        prompts: deps.prompts.clone(),
        setup: deps.setup,
        actors: deps.actors,
        log: log.clone(),
        aborts: aborts.clone(),
        notifier: deps.notifier.clone(),
        clock: deps.clock.clone(),
        ids: deps.ids.clone(),
    });
    let pass = DispatchPass::new(
        deps.config.clone(),
        state.clone(),
        deps.clock.clone(),
        deps.local_time.clone(),
        log.clone(),
    );
    let research = ResearchRuns::new(ResearchParts {
        questions: deps.questions,
        tickets: deps.tickets.clone(),
        comments: deps.comments.clone(),
        boards: deps.boards.clone(),
        runners: deps.runners.clone(),
        runs: deps.runs.clone(),
        dispatch: deps.dispatch.clone(),
        prompts: deps.prompts.clone(),
        detached: deps.detached.clone(),
        tracking: deps.tracking.clone(),
        accounts: accounts.clone(),
        state: state.clone(),
        config: deps.config.clone(),
        pass: pass.clone(),
        local: deps.local_time.clone(),
        starter: starter.clone(),
        log: log.clone(),
        clock: deps.clock.clone(),
        notifier: deps.notifier.clone(),
    });
    let merges = Arc::new(MergeQueue::new(
        deps.merge_jobs,
        deps.worktree_repository.clone(),
        deps.config.clone(),
        deps.clock.clone(),
        deps.ids.clone(),
        deps.signal.clone(),
    ));
    let planning = PlanningRuns::new(PlanningParts {
        tickets: deps.tickets.clone(),
        comments: deps.comments.clone(),
        boards: deps.boards.clone(),
        runners: deps.runners.clone(),
        runs: deps.runs.clone(),
        dispatch: deps.dispatch.clone(),
        prompts: deps.prompts.clone(),
        detached: deps.detached.clone(),
        tracking: deps.tracking.clone(),
        accounts: accounts.clone(),
        state: state.clone(),
        config: deps.config.clone(),
        pass: pass.clone(),
        local: deps.local_time.clone(),
        starter: starter.clone(),
        log: log.clone(),
        clock: deps.clock.clone(),
    });
    let merge_all = Arc::new(
        MergeAll::new(MergeParts {
            queue: merges.clone(),
            worktrees: deps.worktree_repository.clone(),
            worktree_service: deps.worktrees.clone(),
            tickets: deps.tickets.clone(),
            comments: deps.comments.clone(),
            runners: deps.runners.clone(),
            git: deps.merge_git.clone(),
            starter: starter.clone(),
            log: log.clone(),
            clock: deps.clock.clone(),
            signal: deps.signal.clone(),
            runs: deps.runs.clone(),
            dispatch: deps.dispatch.clone(),
            prompts: deps.prompts.clone(),
            accounts: accounts.clone(),
        })
        .with_notifier(deps.notifier.clone()),
    );
    let evaluate = Arc::new(
        EvaluateRuns::new(
            deps.tickets.clone(),
            deps.comments.clone(),
            deps.runs.clone(),
            starter.clone(),
            log.clone(),
            aborts.clone(),
        )
        .with_accounts(accounts.clone())
        .with_notifier(deps.notifier.clone())
        .with_gate(Arc::new(MergeGate::new(
            Arc::new(
                ReviewGate::new(
                    deps.reviews.clone(),
                    deps.tickets.clone(),
                    deps.comments.clone(),
                    deps.runs.clone(),
                    starter.clone(),
                )
                .with_notifier(deps.notifier.clone()),
            ),
            merges.clone(),
        )))
        .with_research(research.clone())
        .with_planning(planning.clone())
        .with_merge(merge_all.clone()),
    );
    evaluate.set_pause_hook(Arc::new(DispatchPauses::new(PauseParts {
        tickets: deps.tickets.clone(),
        comments: deps.comments.clone(),
        runs: deps.runs.clone(),
        accounts: deps.accounts.clone(),
        pauses: pauses.clone(),
        limits: deps.limits.clone(),
        state: state.clone(),
        starter: starter.clone(),
        local: deps.local_time.clone(),
        clock: deps.clock.clone(),
        log: log.clone(),
        notifier: deps.notifier.clone(),
    })));
    pass.set_step(PassPhase::Evaluate, evaluate.clone());
    pass.set_step(
        PassPhase::ResearchOrphans,
        Arc::new(ResearchOrphans(research.clone())),
    );
    pass.set_step(
        PassPhase::PlanOrphans,
        Arc::new(PlanOrphans(planning.clone())),
    );
    pass.set_step(
        PassPhase::ReleaseStale,
        Arc::new(ReleaseStale::new(
            deps.tickets.clone(),
            aborts.clone(),
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
    pass.set_step(
        PassPhase::StartResearch,
        Arc::new(StartResearch(research.clone())),
    );
    pass.set_step(
        PassPhase::MergeCatchUp,
        Arc::new(MergeCatchUp::new(
            merges.clone(),
            deps.tickets.clone(),
            deps.worktree_repository.clone(),
            deps.runners.clone(),
            deps.runs.clone(),
            deps.merge_git.clone(),
        )),
    );
    pass.set_step(PassPhase::MergeAll, merge_all);
    pass.set_step(
        PassPhase::StartPlanning,
        Arc::new(StartPlanning(planning.clone())),
    );
    let decisions = Arc::new(
        ReviewDecisions::new(DecisionParts {
            pass: pass.clone(),
            tickets: deps.tickets.clone(),
            comments: deps.comments.clone(),
            worktrees: deps.worktrees,
            reviews: deps.reviews,
            runs: deps.runs.clone(),
            starter: starter.clone(),
        })
        .with_hook(Arc::new(QueueOnAccept(merges.clone()))),
    );
    let retries = Arc::new(MergeRetry::new(RetryParts {
        pass: pass.clone(),
        queue: merges.clone(),
        tickets: deps.tickets.clone(),
        comments: deps.comments.clone(),
        worktrees: deps.worktree_repository.clone(),
        runs: deps.runs.clone(),
        starter: starter.clone(),
        clock: deps.clock.clone(),
        signal: deps.signal.clone(),
    }));
    let service = Arc::new(DispatcherService::new(ControlParts {
        research: research.clone(),
        planning: planning.clone(),
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
        evaluate,
        research,
        planning,
        decisions,
        merges,
        retries,
    }
}
