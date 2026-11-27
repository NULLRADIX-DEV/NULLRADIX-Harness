//! Merge jobs: the branch of a worktree goes into its target branch, one job per worktree.

use strum::{EnumString, IntoStaticStr};

use crate::{Id, MERGE_REASON, MergeStrategy, Timestamp, Version, Worktree};

/// Longest block reason a merge writes on a ticket.
pub const MERGE_REASON_MAX: usize = 500;
/// Longest problem a job keeps.
pub const MERGE_PROBLEM_MAX: usize = 2000;
/// Commits a squash lists in its message at most.
pub const SQUASH_LIST_MAX: usize = 50;
/// Minutes a merge waits for its folder before the person is told once.
pub const HOLD_NOTICE_MINUTES: i64 = 15;
/// Files a hold names before it counts the rest.
pub const HOLD_FILES_MAX: usize = 10;

/// Why a merge waits for its folder instead of failing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum HoldKind {
    /// A merge, rebase, cherry-pick or revert is under way.
    Busy,
    /// A merge worktree of ours still has changes of an earlier attempt.
    Leftover,
    /// Changes are staged in the folder.
    Staged,
    /// Local changes touch files the branch changes too.
    Overlap,
}

impl HoldKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Up to [`HOLD_FILES_MAX`] files, then how many more.
#[must_use]
pub fn hold_files(files: &[String]) -> String {
    if files.is_empty() {
        return String::from("unknown files");
    }
    if files.len() > HOLD_FILES_MAX {
        return format!(
            "{} and {} more",
            files[..HOLD_FILES_MAX].join(", "),
            files.len() - HOLD_FILES_MAX
        );
    }
    files.join(", ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum MergePhase {
    Pending,
    Waiting,
    ConflictSession,
    Merged,
    Failed,
    Cancelled,
}

impl MergePhase {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// Whether the job still has to be processed.
    #[must_use]
    pub const fn active(self) -> bool {
        matches!(self, Self::Pending | Self::Waiting | Self::ConflictSession)
    }
}

/// Every stored field of a merge job, for persistence adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the record mirrors the flags of the table"
)]
pub struct MergeJobRecord {
    pub id: Id<MergeJob>,
    pub worktree: Id<Worktree>,
    pub phase: MergePhase,
    pub target_branch: Option<String>,
    pub target_head: Option<String>,
    pub session_branch_head: Option<String>,
    pub strategy: MergeStrategy,
    pub conflict_sessions: u32,
    pub session_used: bool,
    pub conflict_seen: bool,
    pub wait_noted: bool,
    pub held_by: Option<String>,
    pub hold_noticed: bool,
    pub problem: Option<String>,
    pub merged_into: Option<String>,
    pub merged_commit: Option<String>,
    pub since: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub version: Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeJob(MergeJobRecord);

impl MergeJob {
    /// A job that waits to be processed since `at`.
    #[must_use]
    pub const fn pending(
        id: Id<Self>,
        worktree: Id<Worktree>,
        target_branch: Option<String>,
        strategy: MergeStrategy,
        at: Timestamp,
    ) -> Self {
        Self(MergeJobRecord {
            id,
            worktree,
            phase: MergePhase::Pending,
            target_branch,
            target_head: None,
            session_branch_head: None,
            strategy,
            conflict_sessions: 0,
            session_used: false,
            conflict_seen: false,
            wait_noted: false,
            held_by: None,
            hold_noticed: false,
            problem: None,
            merged_into: None,
            merged_commit: None,
            since: at,
            finished_at: None,
            version: Version::INITIAL,
        })
    }

    /// Trusts a record read back from storage.
    #[must_use]
    pub const fn restore(record: MergeJobRecord) -> Self {
        Self(record)
    }

    #[must_use]
    pub const fn record(&self) -> &MergeJobRecord {
        &self.0
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.0.id
    }

    /// The job waits until the work is ready or the folder is free.
    #[must_use]
    pub fn waiting(mut self, noted: bool) -> Self {
        if self.held_text().is_some() {
            self.0.problem = None;
        }
        self.0.phase = MergePhase::Waiting;
        self.0.wait_noted |= noted;
        self
    }

    /// The job waits for its folder; `text` says where and why, `noticed` that the person was told.
    #[must_use]
    pub fn held(mut self, kind: HoldKind, text: &str, noticed: bool) -> Self {
        self.0.phase = MergePhase::Waiting;
        self.0.held_by = Some(kind.as_str().to_owned());
        self.0.hold_noticed |= noticed;
        self.0.problem = Some(cap_problem(text));
        self
    }

    /// Whether the job already waits for exactly this text, as `held` stores it.
    #[must_use]
    pub fn waits_for(&self, text: &str) -> bool {
        self.held_text() == Some(cap_problem(text).as_str())
    }

    /// What the job waits for in its folder, while it does.
    #[must_use]
    pub fn held_text(&self) -> Option<&str> {
        let record = &self.0;
        if record.phase == MergePhase::Waiting && !record.conflict_seen && record.held_by.is_some()
        {
            record.problem.as_deref()
        } else {
            None
        }
    }

    /// The job is ready to be processed again.
    #[must_use]
    pub fn pending_again(mut self) -> Self {
        self.0.phase = MergePhase::Pending;
        self
    }

    /// The merge conflicted and was undone; the job keeps the files for a merge session.
    #[must_use]
    pub fn conflicted(mut self, files: &[String]) -> Self {
        self.0.conflict_seen = true;
        self.0.problem = Some(conflict_problem(files));
        self
    }

    /// Session `sessions` runs for the conflict; the target stood at `head`, the branch at `tip`.
    #[must_use]
    pub fn session_started(
        mut self,
        sessions: u32,
        head: &str,
        tip: &str,
        files: &[String],
    ) -> Self {
        self.0.phase = MergePhase::ConflictSession;
        self.0.session_used = true;
        self.0.conflict_sessions = sessions;
        self.0.target_head = Some(head.to_owned());
        self.0.session_branch_head = Some(tip.to_owned());
        self.0.problem = Some(conflict_problem(files));
        self
    }

    /// The session merged the target into the branch; a target that moved on starts afresh.
    #[must_use]
    pub fn session_resolved(mut self, target_moved: bool) -> Self {
        self.0.phase = MergePhase::Pending;
        self.0.problem = None;
        if target_moved {
            self.0.session_used = false;
            self.0.conflict_sessions = 0;
            self.0.target_head = None;
            self.0.session_branch_head = None;
        }
        self
    }

    /// The session failed and autopilot allows another: the next pass merges and starts it.
    #[must_use]
    pub fn next_session(mut self) -> Self {
        self.0.phase = MergePhase::Pending;
        self.0.session_used = false;
        self.0.target_head = None;
        self.0.session_branch_head = None;
        self
    }

    /// The branch is in the target.
    #[must_use]
    pub fn merged(
        mut self,
        into: &str,
        commit: &str,
        strategy: MergeStrategy,
        at: Timestamp,
    ) -> Self {
        self.0.phase = MergePhase::Merged;
        self.0.merged_into = Some(into.to_owned());
        self.0.merged_commit = Some(commit.to_owned());
        self.0.strategy = strategy;
        self.0.problem = None;
        self.0.finished_at = Some(at);
        self
    }

    /// The merge cannot go on without a person.
    #[must_use]
    pub fn failed(mut self, problem: &str, at: Timestamp) -> Self {
        self.0.phase = MergePhase::Failed;
        self.0.problem = Some(problem.chars().take(MERGE_PROBLEM_MAX).collect());
        self.0.finished_at = Some(at);
        self
    }

    /// Whether a retry would change nothing: pending, with no conflict or hold left over.
    #[must_use]
    pub fn pristine(&self) -> bool {
        let record = &self.0;
        record.phase == MergePhase::Pending
            && !record.conflict_seen
            && !record.session_used
            && !record.wait_noted
            && !record.hold_noticed
            && record.conflict_sessions == 0
            && record.target_head.is_none()
            && record.session_branch_head.is_none()
            && record.held_by.is_none()
            && record.problem.is_none()
    }

    /// A person retries: everything a conflict or a wait left behind is forgotten.
    #[must_use]
    pub fn retried(mut self) -> Self {
        self.0.phase = MergePhase::Pending;
        self.0.conflict_seen = false;
        self.0.session_used = false;
        self.0.conflict_sessions = 0;
        self.0.wait_noted = false;
        self.0.hold_noticed = false;
        self.0.target_head = None;
        self.0.session_branch_head = None;
        self.0.held_by = None;
        self.0.problem = None;
        self
    }

    /// Nothing is left to merge.
    #[must_use]
    pub fn cancelled(mut self, at: Timestamp) -> Self {
        self.0.phase = MergePhase::Cancelled;
        self.0.finished_at = Some(at);
        self
    }
}

fn cap_problem(text: &str) -> String {
    text.chars().take(MERGE_PROBLEM_MAX).collect()
}

#[must_use]
pub fn conflict_problem(files: &[String]) -> String {
    if files.is_empty() {
        return String::from("merge conflict");
    }
    let mut text = format!("merge conflict in {}", files.join(", "));
    if text.chars().count() > MERGE_PROBLEM_MAX {
        text = text.chars().take(MERGE_PROBLEM_MAX).collect();
    }
    text
}

/// The block reason a failed merge leaves on its tickets: one line, within the limit.
#[must_use]
pub fn merge_block_reason(problem: &str) -> String {
    let line = problem.split_whitespace().collect::<Vec<_>>().join(" ");
    let full = format!("{MERGE_REASON} {line}");
    if full.chars().count() <= MERGE_REASON_MAX {
        return full;
    }
    let mut cut: String = full.chars().take(MERGE_REASON_MAX - 1).collect();
    cut.push('\u{2026}');
    cut
}

/// The commit message of a merge: one ticket names its title, several only their keys.
#[must_use]
pub fn merge_message(
    tickets: &[(String, String)],
    branch: &str,
    strategy: MergeStrategy,
) -> String {
    let head = match tickets {
        [(key, title)] => format!("{key} \"{title}\""),
        _ => tickets
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    };
    match strategy {
        MergeStrategy::Squash => format!("{head} (squash of {branch})"),
        MergeStrategy::Merge => format!("Merge {head} ({branch})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tickets(count: usize) -> Vec<(String, String)> {
        (1..=count)
            .map(|n| (format!("ARC-{n:04}"), format!("Title {n}")))
            .collect()
    }

    #[test]
    fn the_message_names_the_title_of_a_single_ticket() {
        assert_eq!(
            merge_message(&tickets(1), "ticket/a", MergeStrategy::Merge),
            "Merge ARC-0001 \"Title 1\" (ticket/a)"
        );
        assert_eq!(
            merge_message(&tickets(2), "ticket/a", MergeStrategy::Merge),
            "Merge ARC-0001, ARC-0002 (ticket/a)"
        );
        assert_eq!(
            merge_message(&tickets(1), "ticket/a", MergeStrategy::Squash),
            "ARC-0001 \"Title 1\" (squash of ticket/a)"
        );
    }

    #[test]
    fn a_hold_keeps_its_kind_and_forgets_its_text_when_the_job_waits_for_tickets() {
        let job = MergeJob::pending(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            Id::from_uuid(uuid::Uuid::from_u128(2)),
            None,
            MergeStrategy::Merge,
            Timestamp::from_millis(1),
        );
        let held = job.held(HoldKind::Staged, "main checkout: staged changes", true);
        assert_eq!(held.record().held_by.as_deref(), Some("staged"));
        assert_eq!(held.held_text(), Some("main checkout: staged changes"));
        assert!(held.record().hold_noticed);
        assert!(!held.pristine());
        let waiting = held.waiting(true);
        assert_eq!(waiting.held_text(), None);
        assert_eq!(waiting.record().held_by.as_deref(), Some("staged"));
        assert_eq!(waiting.record().problem, None);
        assert_eq!(HoldKind::Overlap.as_str(), "overlap");
    }

    #[test]
    fn a_long_hold_text_still_matches_the_stored_one() {
        let job = MergeJob::pending(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            Id::from_uuid(uuid::Uuid::from_u128(2)),
            None,
            MergeStrategy::Merge,
            Timestamp::from_millis(1),
        );
        let long = "x".repeat(MERGE_PROBLEM_MAX + 50);
        let held = job.held(HoldKind::Overlap, &long, false);
        assert!(held.waits_for(&long));
        assert!(!held.waits_for("other"));
    }

    #[test]
    fn a_hold_names_ten_files_and_counts_the_rest() {
        assert_eq!(hold_files(&[]), "unknown files");
        let many: Vec<String> = (1..=12).map(|n| format!("f{n}")).collect();
        assert_eq!(
            hold_files(&many),
            "f1, f2, f3, f4, f5, f6, f7, f8, f9, f10 and 2 more"
        );
        assert_eq!(hold_files(&many[..2]), "f1, f2");
    }

    #[test]
    fn a_block_reason_is_one_line_within_its_limit() {
        assert_eq!(merge_block_reason("a\n  b"), "Merge: a b");
        let long = merge_block_reason(&"x".repeat(900));
        assert_eq!(long.chars().count(), MERGE_REASON_MAX);
        assert!(long.starts_with("Merge: x"));
    }

    fn pending_job() -> MergeJob {
        MergeJob::pending(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            Id::from_uuid(uuid::Uuid::from_u128(2)),
            None,
            MergeStrategy::Merge,
            Timestamp::from_millis(1),
        )
    }

    #[test]
    fn a_session_marks_the_job_and_a_failed_one_makes_room_for_the_next() {
        let files = [String::from("a.rs")];
        let started = pending_job().session_started(2, "abc123", "def456", &files);
        let record = started.record();
        assert_eq!(record.phase, MergePhase::ConflictSession);
        assert!(record.phase.active() && record.session_used);
        assert_eq!(record.conflict_sessions, 2);
        assert_eq!(record.target_head.as_deref(), Some("abc123"));
        assert_eq!(record.session_branch_head.as_deref(), Some("def456"));
        assert_eq!(record.problem.as_deref(), Some("merge conflict in a.rs"));
        let next = started.clone().next_session();
        let record = next.record();
        assert_eq!(record.phase, MergePhase::Pending);
        assert!(!record.session_used && record.target_head.is_none());
        assert!(record.session_branch_head.is_none());
        assert_eq!(record.conflict_sessions, 2);
    }

    #[test]
    fn a_resolved_session_keeps_its_count_unless_the_target_moved() {
        let started = pending_job().session_started(2, "abc123", "def456", &[]);
        let kept = started.clone().session_resolved(false);
        assert_eq!(kept.record().phase, MergePhase::Pending);
        assert!(kept.record().session_used);
        assert_eq!(kept.record().conflict_sessions, 2);
        assert_eq!(kept.record().problem, None);
        let moved = started.session_resolved(true);
        assert!(!moved.record().session_used);
        assert_eq!(moved.record().conflict_sessions, 0);
        assert_eq!(moved.record().target_head, None);
        assert_eq!(moved.record().session_branch_head, None);
        assert!(moved.pristine());
    }

    #[test]
    fn a_job_ends_merged_failed_or_cancelled() {
        let job = MergeJob::pending(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            Id::from_uuid(uuid::Uuid::from_u128(2)),
            None,
            MergeStrategy::Merge,
            Timestamp::from_millis(1),
        );
        assert!(job.record().phase.active());
        let conflicted = job.clone().conflicted(&[String::from("a.rs")]);
        assert!(conflicted.record().conflict_seen);
        assert_eq!(
            conflicted.record().problem.as_deref(),
            Some("merge conflict in a.rs")
        );
        let merged = job.clone().merged(
            "main",
            "abc",
            MergeStrategy::Squash,
            Timestamp::from_millis(2),
        );
        assert_eq!(merged.record().phase, MergePhase::Merged);
        assert_eq!(merged.record().strategy, MergeStrategy::Squash);
        assert!(!merged.record().phase.active());
        let retried = conflicted.clone().waiting(true).retried();
        assert!(retried.pristine());
        assert!(!conflicted.pristine());
        let failed = job.failed("no", Timestamp::from_millis(3));
        assert_eq!(failed.record().phase, MergePhase::Failed);
        assert_eq!(MergePhase::ConflictSession.as_str(), "conflict_session");
        assert_eq!(conflict_problem(&[]), "merge conflict");
    }
}
