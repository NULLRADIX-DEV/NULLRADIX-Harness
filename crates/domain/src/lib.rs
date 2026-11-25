//! Entities and rules of NULLRADIX, free of IO.

mod account;
mod board;
mod board_rules;
mod comment;
mod customization;
mod dispatcher_config;
mod favorite;
mod hook;
mod id;
mod inbox;
mod note;
mod project;
mod question;
mod repository;
mod run;
mod session;
pub mod step;
mod text;
mod theme;
mod ticket;
mod ticket_values;
mod timestamp;
mod version;
mod worktree;

pub use account::{
    ACCOUNT_NAME_MAX, ACCOUNT_SLUG_MAX, Account, AccountChanges, AccountError, AccountName,
    AccountSlug, CONFIG_DIR_MAX, ConfigDir, DEFAULT_SLUG, Limit, RunToken, TOKEN_PREFIX,
};
pub use board::{Actor, ActorKind, ActorName, Board, BoardError, BoardName, BoardRecord};
pub use board_rules::{
    DependencyIndex, LABEL_MAX, RuleError, display_order, normalize_dependencies, normalize_labels,
    validate_dependencies,
};
pub use comment::{Comment, CommentBody, CommentError, CommentOn, CommentRecord, SystemMarker};
pub use customization::{
    Base, BaseState, ExtensionPoint, Finding, FrontMatter, OVERLAY_CONTENT_MAX, OVERLAY_NAME_MAX,
    Overlay, OverlayError, OverlayKind, OverlayName, OverlayRecord, OverlayState, base_state,
    check_front_matter, overlay_content, overlay_state,
};
pub use dispatcher_config::{
    AccountStrategy, AutopilotSettings, ClaudeSettings, ConfigError, ConfigFieldError,
    DEFAULT_ALLOWED_TOOLS, DispatcherConfig, EFFORT_LEVELS, FALLBACK_PROFILE, FINGERPRINT_HASH_LEN,
    Fingerprint, JoinWorktrees, MAX_PARALLEL_AUTO_RUNS, MODEL_ALIASES, MergeStrategy,
    PROFILE_NAME_MAX, PlanningSettings, Profile, PromptKind, PromptTemplates, ResearchSettings,
    Rule, RuleCondition, TimeWindow, WorktreeSetupStep, effort_problem, model_problem,
};
pub use favorite::{
    FAVORITE_NOTE_MAX, Favorite, FavoriteRecord, Resumption, favorite_note, resume_command,
    resumed_favorite,
};
pub use hook::{HookDetails, HookError, HookEvent, HookKind, SessionRef};
pub use id::{Id, ParseIdError};
pub use inbox::{
    INBOX_DETAIL_MAX, INBOX_ORDER_MAX, INBOX_TITLE_MAX, InboxCounts, InboxError, InboxItem,
    InboxKind, InboxRef, SETUP_REASON, apply_order, clip, compare, note_text, order,
};
pub use note::{
    NOTE_LABELS_MAX, NOTE_REASON_MAX, NOTE_SEEN_IN_MAX, NOTE_TITLE_MAX, Note, NoteDraft, NoteError,
    NoteState, Sighting, TicketRef, check_note_labels, drop_reason, note_number, note_title,
    shown_title, similar_notes,
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
pub use session::{
    DisplayState, SESSION_RETENTION, SESSION_STALE, Session, SessionClient, SessionRecord,
    SessionState, SessionView, Subagent, SubagentState, path_within, session_label,
    session_prune_before, ticket_names, title_from_prompt,
};
pub use step::{STEP_MAX, Step, StepEdit, StepError};
pub use text::{TextError, optional_text, required_text, utf16_len, utf16_prefix};
pub use theme::{
    BUILTIN_THEMES, CLAUDE_DIR, CLAUDE_HOOKS_FILE, ColorScheme, DEFAULT_THEME, FieldError,
    InstalledTheme, InstalledThemeRecord, MANIFEST_FILE, MAX_ENTRIES, MAX_ENTRY_PATH,
    MAX_FILE_BYTES, MAX_PACKAGE_BYTES, ManifestDraft, ManifestRecord, PackageError, STYLE_FILE,
    SelectOption, SettingSpec, ThemeError, ThemeId, ThemeManifest, asset_segments,
    check_claude_files, check_entry_path, check_theme_css, content_type, host_allowed,
    is_allowed_extension, is_claude_path, is_hidden,
};
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
