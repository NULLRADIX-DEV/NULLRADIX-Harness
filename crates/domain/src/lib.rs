//! Entities and rules of NULLRADIX, free of IO.

mod board;
mod board_rules;
mod hook;
mod id;
mod project;
mod run;
mod text;
mod ticket;
mod timestamp;
mod version;

pub use board::{Actor, ActorKind, ActorName, Board, BoardError, BoardName, BoardRecord};
pub use board_rules::{
    LABEL_MAX, RuleError, normalize_dependencies, normalize_labels, validate_dependencies,
};
pub use hook::{HookDetails, HookError, HookEvent, HookKind, SessionRef};
pub use id::{Id, ParseIdError};
pub use project::{Project, ProjectError, ProjectKey, ProjectName};
pub use run::{Run, RunEnd, RunError, RunLabel, RunOutcome, RunRecord, RunStatus};
pub use text::{TextError, required_text, utf16_len};
pub use ticket::{
    Column, Priority, StatusFacts, StatusLock, StatusRequest, TicketError, TicketKey, TicketStatus,
    check_status_change,
};
pub use timestamp::Timestamp;
pub use version::{InvalidVersion, Version};
