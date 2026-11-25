//! Entities and rules of NULLRADIX, free of IO.

mod board;
mod board_rules;
mod comment;
mod hook;
mod id;
mod note;
mod project;
mod question;
mod repository;
mod run;
pub mod step;
mod text;
mod ticket;
mod ticket_values;
mod timestamp;
mod version;
mod worktree;

pub use board::{Actor, ActorKind, ActorName, Board, BoardError, BoardName, BoardRecord};
pub use board_rules::{
    DependencyIndex, LABEL_MAX, RuleError, display_order, normalize_dependencies, normalize_labels,
    validate_dependencies,
};
pub use comment::{Comment, CommentBody, CommentError, CommentOn, CommentRecord, SystemMarker};
pub use hook::{HookDetails, HookError, HookEvent, HookKind, SessionRef};
pub use id::{Id, ParseIdError};
pub use note::{
    NOTE_LABELS_MAX, NOTE_REASON_MAX, NOTE_SEEN_IN_MAX, NOTE_TITLE_MAX, Note, NoteDraft, NoteError,
    NoteState, Sighting, TicketRef, check_note_labels, drop_reason, note_number, note_title,
    similar_notes,
};
pub use project::{Project, ProjectError, ProjectKey, ProjectName};
pub use question::{
    AUTOPILOT_PREFIX, AnswerInput, Choice, Decision, DecisionError, OCCASION_MAX, OPTION_TITLE_MAX,
    OPTIONS_MAX, OptionDraft, QUESTION_TITLE_MAX, Question, QuestionDraft, QuestionError,
    QuestionKey, QuestionOption, QuestionRecord, QuestionRef, QuestionState, QuestionTitle,
    Research, ResearchError, SESSION_MAX, describe, number_options, occasion, reopen_note, session,
    ticket_note,
};
pub use repository::{
    CHECKOUT_PATH_MAX, Checkout, CheckoutPath, CheckoutRecord, REPOSITORY_NAME_MAX,
    RUNNER_NAME_MAX, Repository, RepositoryError, RepositoryName, RepositoryRecord, Runner,
    RunnerName, RunnerOs, RunnerRecord,
};
pub use run::{Run, RunEnd, RunError, RunLabel, RunOutcome, RunRecord, RunStatus};
pub use step::{STEP_MAX, Step, StepEdit, StepError};
pub use text::{TextError, optional_text, required_text, utf16_len, utf16_prefix};
pub use ticket::{
    ASSIGNEE_MAX, BLOCKED_REASON_MAX, BRANCH_MAX, FieldChange, HUMAN_INTERACTION, HistoryEntry,
    PLANNING_CLAIM_TTL, TITLE_MAX, TargetBranch, Ticket, TicketDraft, TicketRecord, TicketTitle,
    Tombstone, changes, check_target_lock, planning_candidate, planning_claim_alive, strip_fence,
    waits_for_a_person,
};
pub use ticket_values::{
    Column, Priority, StatusFacts, StatusLock, StatusRequest, TicketError, TicketKey, TicketStatus,
    check_status_change,
};
pub use timestamp::Timestamp;
pub use version::{InvalidVersion, Version};
pub use worktree::{
    ActualState, DesiredState, JoinCandidate, MERGE_REASON, SetupState, WORKTREE_BRANCH_MAX,
    WORKTREE_PATH_MAX, Worktree, WorktreeBranch, WorktreeDraft, WorktreeError, WorktreePath,
    WorktreePurpose, WorktreeRecord, WorktreeTicket, finished_for_merge, pick_join, ready_to_merge,
    slug, ticket_branch, ticket_worktree_path,
};
