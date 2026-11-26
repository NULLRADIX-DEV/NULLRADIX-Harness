//! What the dispatcher decides before it starts a ticket: readiness, profile, account, state.

mod account_choice;
mod attempts;
mod escalation;
mod profile_choice;
mod readiness;
mod state;
mod usage_estimate;
mod window;

pub use account_choice::{
    ACCOUNT_LABEL_PREFIX, AccountChoice, AccountChoiceInput, AccountReserve, AccountState,
    AllowedAccounts, UsageSnapshot, account_label, allowed_accounts, choose_account, clock_text,
};
pub use attempts::{ABORT_PREFIX, failed_attempts};
pub use escalation::{LabelOverrides, escalate, label_overrides, label_problem};
pub use profile_choice::{
    PROFILE_LABEL_PREFIX, ProfileChoice, ProfileReason, choose_profile, plan_estimate,
    plan_file_count,
};
pub use readiness::{NotReady, ReadinessFacts, RepositoryFact, not_ready};
pub use state::{
    DispatcherPhase, DispatcherState, DispatcherStateRecord, IdleReason, idle_reason, phase,
};
pub use usage_estimate::{
    EstimatedReserve, MAX_SAMPLES, MIN_SAMPLES, ReserveFor, RunningWork, UsageSample,
    estimate_reserve, pending_reserve,
};
pub use window::minutes_of_day;
