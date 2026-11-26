//! Merge jobs: the branch of a worktree goes into its target branch, one job per worktree.

use strum::{EnumString, IntoStaticStr};

use crate::{Id, MERGE_REASON, MergeStrategy, Timestamp, Version, Worktree};

/// Longest block reason a merge writes on a ticket.
pub const MERGE_REASON_MAX: usize = 500;
/// Longest problem a job keeps.
pub const MERGE_PROBLEM_MAX: usize = 2000;
/// Commits a squash lists in its message at most.
pub const SQUASH_LIST_MAX: usize = 50;

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
        self.0.phase = MergePhase::Waiting;
        self.0.wait_noted |= noted;
        self
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

    /// Nothing is left to merge.
    #[must_use]
    pub fn cancelled(mut self, at: Timestamp) -> Self {
        self.0.phase = MergePhase::Cancelled;
        self.0.finished_at = Some(at);
        self
    }
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
    fn a_block_reason_is_one_line_within_its_limit() {
        assert_eq!(merge_block_reason("a\n  b"), "Merge: a b");
        let long = merge_block_reason(&"x".repeat(900));
        assert_eq!(long.chars().count(), MERGE_REASON_MAX);
        assert!(long.starts_with("Merge: x"));
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
        let failed = job.failed("no", Timestamp::from_millis(3));
        assert_eq!(failed.record().phase, MergePhase::Failed);
        assert_eq!(MergePhase::ConflictSession.as_str(), "conflict_session");
        assert_eq!(conflict_problem(&[]), "merge conflict");
    }
}
