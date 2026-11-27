//! Entities and rules of NULLRADIX, free of IO.

mod account;
mod board;
mod board_rules;
mod comment;
mod css_check;
mod customization;
mod dispatch_pass;
mod dispatch_run;
mod dispatch_stats;
mod dispatcher_config;
mod favorite;
mod hook;
mod id;
mod inbox;
mod merge;
mod merge_session;
mod note;
mod project;
mod question;
mod repository;
mod review;
mod run;
mod session;
mod snippet;
pub mod step;
mod text;
mod theme;
mod theme_catalog;
mod theme_fetch;
mod theme_risk;
mod theme_settings;
mod ticket;
mod ticket_transitions;
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
    DependencyIndex, LABEL_MAX, RuleError, changed_dependencies, display_order,
    normalize_dependencies, normalize_labels, validate_dependencies,
};
pub use comment::{Comment, CommentBody, CommentError, CommentOn, CommentRecord, SystemMarker};
pub use customization::{
    Base, BaseState, ExtensionPoint, Finding, FrontMatter, OVERLAY_CONTENT_MAX, OVERLAY_NAME_MAX,
    Overlay, OverlayError, OverlayKind, OverlayName, OverlayRecord, OverlayState, base_state,
    check_front_matter, overlay_content, overlay_state, refused_keys,
};
pub use dispatch_pass::{
    ABORT_PREFIX, ACCOUNT_LABEL_PREFIX, AccountChoice, AccountChoiceInput, AccountReserve,
    AccountState, AllowedAccounts, DispatcherPhase, DispatcherState, DispatcherStateRecord,
    EstimatedReserve, IdleReason, LabelOverrides, MAX_SAMPLES, MIN_SAMPLES, NotReady,
    PROFILE_LABEL_PREFIX, ProfileChoice, ProfileReason, ReadinessFacts, RepositoryFact, ReserveFor,
    RunningWork, UsageSample, UsageSnapshot, account_label, allowed_accounts, choose_account,
    choose_profile, clock_text, escalate, estimate_reserve, failed_attempts, idle_reason, is_abort,
    is_abort_of, label_overrides, label_problem, minutes_of_day, not_ready, pending_reserve, phase,
    plan_estimate, plan_file_count,
};
pub use dispatch_run::{
    Abort, AbortCategory, DeniedTool, DispatchOutcome, DispatchRun, ModelUsage, RunKind,
    RunProfile, RunResult, RunTrigger, RunUsage, Screenshot,
};
pub use dispatch_stats::{
    AbortReason, DispatchStats, OutcomeCount, ProfileRow, STATS_DEFAULT_DAYS, STATS_MAX_DAYS,
    STATS_MAX_RUNS, SeriesPoint, SeriesUnit, StatsError, StatsFacts, StatsKpis, StatsLadder,
    StatsRange, StatsRun, StatsSeries, StatsUsage, abort_label, calculate_stats, day_label,
    local_day_of, parse_stats_kind, wants_config,
};
pub use dispatcher_config::{
    AccountStrategy, AnsweredQuestion, AutopilotSettings, ClaudeSettings, ConfigError,
    ConfigFieldError, DEFAULT_ALLOWED_TOOLS, DispatcherConfig, EFFORT_LEVELS, FALLBACK_PROFILE,
    FINGERPRINT_HASH_LEN, Fingerprint, JoinWorktrees, MAX_PARALLEL_AUTO_RUNS, MODEL_ALIASES,
    MergeRun, MergeStrategy, PLAN_AGENT_PREFIX, PROFILE_NAME_MAX, PlaceholderInfo, PlanRun,
    PlanningSettings, Profile, PromptKind, PromptRun, PromptTemplates, PromptValues,
    RESEARCH_AGENT_PREFIX, RequiredPart, ResearchRun, ResearchSettings, Rule, RuleCondition,
    TimeWindow, UnknownPlaceholders, WorkRun, WorktreeSetupStep, default_template, effort_problem,
    missing_parts, model_problem, placeholder_names, placeholders, render_prompt, render_template,
    required_parts, size_problem, unknown_placeholders,
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
pub use merge::{
    HOLD_FILES_MAX, HOLD_NOTICE_MINUTES, HoldKind, MERGE_PROBLEM_MAX, MERGE_REASON_MAX, MergeJob,
    MergeJobRecord, MergePhase, SQUASH_LIST_MAX, conflict_problem, hold_files, merge_block_reason,
    merge_message,
};
pub use merge_session::{
    MERGE_CONFLICT_PREFIX, MERGE_SESSIONS_CAP, NEXT_SESSION, PREVIOUS_REPORT_MAX, ReportComment,
    merge_session_choice, merge_session_limit, previous_attempts, session_failed_note,
    session_started_note,
};
pub use note::{
    NOTE_LABELS_MAX, NOTE_REASON_MAX, NOTE_SEEN_IN_MAX, NOTE_TITLE_MAX, Note, NoteDraft, NoteError,
    NoteState, Sighting, TicketRef, check_note_labels, drop_reason, note_number, note_title,
    shown_title, similar_notes,
};
pub use project::{Project, ProjectError, ProjectKey, ProjectName};
pub use question::{
    AUTOPILOT_PREFIX, AnswerInput, Choice, Decision, DecisionError, OCCASION_MAX, OPTION_POINT_MAX,
    OPTION_POINTS_MAX, OPTION_TEXT_MAX, OPTION_TITLE_MAX, OPTIONS_MAX, OptionDraft,
    QUESTION_TITLE_MAX, Question, QuestionDraft, QuestionError, QuestionKey, QuestionOption,
    QuestionRecord, QuestionRef, QuestionState, QuestionTitle, RESEARCH_CONTEXT_MAX, Research,
    ResearchError, SESSION_MAX, describe, number_options, occasion, reopen_note, research_context,
    session, ticket_note,
};
pub use repository::{
    CHECKOUT_PATH_MAX, Checkout, CheckoutPath, CheckoutRecord, REPOSITORY_NAME_MAX,
    RUNNER_NAME_MAX, Repository, RepositoryError, RepositoryName, RepositoryRecord, Runner,
    RunnerName, RunnerOs, RunnerRecord,
};
pub use review::{
    REVIEW_COMMAND_MAX, REVIEW_FEEDBACK_MAX, REVIEW_REASON, REVIEW_RETURNED_PREFIX,
    REVIEW_TESTS_MAX, Review, ReviewRecord, ReviewState, TestRun, change_summary, pending_feedback,
    returned_comment, review_reason, test_summary, waits_for_review,
};
pub use run::{Run, RunEnd, RunError, RunLabel, RunOutcome, RunRecord, RunStatus};
pub use session::{
    DisplayState, SESSION_RETENTION, SESSION_STALE, Session, SessionClient, SessionRecord,
    SessionState, SessionView, Subagent, SubagentState, path_within, session_label,
    session_prune_before, ticket_names, title_from_prompt,
};
pub use snippet::{
    SNIPPET_CSS_MAX, SNIPPET_ID_MAX, SNIPPET_ID_MIN, SNIPPET_NAME_MAX, Snippet, SnippetCss,
    SnippetError, SnippetId, SnippetName, SnippetRecord, active_stylesheet,
};
pub use step::{STEP_MAX, Step, StepEdit, StepError};
pub use text::{TextError, optional_text, required_text, utf16_len, utf16_prefix};
pub use theme::{
    BUILTIN_THEMES, CLAUDE_DIR, CLAUDE_HOOKS_FILE, CODE_FILE, ColorScheme, DEFAULT_THEME,
    FieldError, InstalledTheme, InstalledThemeRecord, MANIFEST_FILE, MAX_ENTRIES, MAX_ENTRY_PATH,
    MAX_FILE_BYTES, MAX_PACKAGE_BYTES, ManifestDraft, ManifestRecord, PackageError, SDK_VERSION,
    STYLE_FILE, SelectOption, SettingSpec, THEME_ID_MAX, ThemeError, ThemeId, ThemeManifest,
    asset_segments, check_claude_files, check_entry_path, check_theme_css, content_type,
    host_allowed, is_allowed_extension, is_claude_path, is_hidden,
};
pub use theme_catalog::{
    Availability, CatalogEntry, CatalogError, CatalogSource, CatalogUrl, Endpoint, EndpointHost,
    EntryDraft, MAX_DESCRIPTION_CHARS, MAX_INDEX_BYTES, MAX_INDEX_ENTRIES, MAX_SOURCES,
    MAX_URL_CHARS, MAX_VERSION_CHARS, Reach, SourceId, is_public_address,
};
pub use theme_fetch::{
    AssetUrl, AssetUrlError, MAX_ARCHIVE_BYTES, MAX_ASSET_BYTES, MAX_CACHE_BYTES, MAX_CACHE_FILES,
    MAX_REDIRECTS, check_asset_entry, foreign_content_type, parse_sha256,
};
pub use theme_risk::{RiskAdded, ThemeRisk, inspect_package};
pub use theme_settings::{SETTING_VALUE_MAX, SettingValue, ThemeSettingValues};
pub use ticket::{
    ASSIGNEE_MAX, BLOCKED_REASON_MAX, BRANCH_MAX, FieldChange, HUMAN_INTERACTION, HistoryEntry,
    PLANNING_CLAIM_TTL, TITLE_MAX, TargetBranch, Ticket, TicketDraft, TicketRecord, TicketTitle,
    Tombstone, changes, check_target_lock, planning_candidate, planning_claim_alive, strip_fence,
    waits_for_a_person,
};
pub use ticket_transitions::{
    AcceptError, BoardMoveError, ClaimError, PlanChange, PlanningClaimed,
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
