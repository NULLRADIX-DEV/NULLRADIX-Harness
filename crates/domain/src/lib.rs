//! Entities and rules of NULLRADIX, free of IO.

mod board;
mod board_rules;
mod hook;
mod id;
mod project;
mod run;
pub mod step;
mod text;
mod ticket;
mod ticket_values;
mod timestamp;
mod version;

pub use board::{Actor, ActorKind, ActorName, Board, BoardError, BoardName, BoardRecord};
pub use board_rules::{
    DependencyIndex, LABEL_MAX, RuleError, display_order, normalize_dependencies, normalize_labels,
    validate_dependencies,
};
pub use hook::{HookDetails, HookError, HookEvent, HookKind, SessionRef};
pub use id::{Id, ParseIdError};
pub use project::{Project, ProjectError, ProjectKey, ProjectName};
pub use run::{Run, RunEnd, RunError, RunLabel, RunOutcome, RunRecord, RunStatus};
pub use step::{STEP_MAX, Step, StepEdit, StepError};
pub use text::{TextError, optional_text, required_text, utf16_len, utf16_prefix};
pub use ticket::{
    ASSIGNEE_MAX, BLOCKED_REASON_MAX, BRANCH_MAX, FieldChange, HistoryEntry, TITLE_MAX,
    TargetBranch, Ticket, TicketDraft, TicketRecord, TicketTitle, Tombstone, changes,
};
pub use ticket_values::{
    Column, Priority, StatusFacts, StatusLock, StatusRequest, TicketError, TicketKey, TicketStatus,
    check_status_change,
};
pub use timestamp::Timestamp;
pub use version::{InvalidVersion, Version};
