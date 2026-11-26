//! What the dispatcher decides before it starts a ticket: readiness, profile, window and state.

mod profile_choice;
mod readiness;
mod state;
mod window;

pub use profile_choice::{
    PROFILE_LABEL_PREFIX, ProfileChoice, ProfileReason, choose_profile, plan_estimate,
    plan_file_count,
};
pub use readiness::{NotReady, ReadinessFacts, RepositoryFact, not_ready};
pub use state::{
    DispatcherPhase, DispatcherState, DispatcherStateRecord, IdleReason, idle_reason, phase,
};
pub use window::minutes_of_day;
