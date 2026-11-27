//! Worktree records: legacy `worktrees.json` with the tickets each worktree served.

mod finish;
mod resume;
mod setup;
mod start;

use async_trait::async_trait;
use nullradix_domain::{
    Id, Repository, Runner, SetupState, Ticket, TicketKey, Timestamp, Worktree, WorktreeBranch,
    WorktreeError, WorktreeTicket,
};

pub use finish::{Cleaned, DoneRequest, Finished};
pub use resume::{Resumed, ResumedWorktree};
pub use start::{StartMode, Started, TargetCreated, WorktreeService};

use crate::{AppError, Change};

/// What the runner needs to add a ticket's worktree; paths are local to the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorktree {
    /// Folder git runs in; without a target the branch starts at its HEAD.
    pub cwd: String,
    /// Main checkout, where a missing target branch is created.
    pub root: String,
    pub branch: String,
    pub path: String,
    pub target: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreatedWorktree {
    /// Where a missing target branch was created from, such as `origin/release` or `main`.
    pub target_created_from: Option<String>,
    /// Whether `core.longpaths` was just switched on in the repository.
    pub long_paths_enabled: bool,
}

/// What the runner needs to remove a ready worktree; the branch always stays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveWorktree {
    /// Main checkout, where git runs.
    pub root: String,
    pub path: String,
    /// Folder of the caller; a worktree containing it stays.
    pub cwd: String,
}

/// The runner's answer to a removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeRemoval {
    Removed,
    /// The folder stays; the reason says why.
    Kept(String),
    /// The folder is gone already; nothing is reported or recorded, as in legacy.
    Missing,
}

/// What the runner needs to bring a ticket's worktree back on its branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureWorktree {
    /// Main checkout, where git runs.
    pub root: String,
    pub branch: String,
    pub path: String,
}

/// The runner's answer to an ensure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsuredWorktree {
    /// The folder was a worktree of the branch already.
    Present,
    /// The folder was added again; a leftover folder moved aside is named.
    Restored { moved_aside: Option<String> },
    /// The branch does not exist in this repository, so nothing was changed.
    BranchMissing,
}

/// A worktree with every ticket it ever served, in join order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub worktree: Worktree,
    pub tickets: Vec<WorktreeTicket>,
}

impl WorktreeEntry {
    /// The ticket that started the worktree.
    #[must_use]
    pub fn owner(&self) -> Option<&WorktreeTicket> {
        self.tickets.first()
    }
}

/// Link changes inside a ticket write, so they commit together with the ticket.
#[async_trait]
pub trait WorktreeWrite: Send {
    /// Stores the worktree with `owner` as its first ticket.
    async fn insert_worktree(
        &mut self,
        worktree: &Worktree,
        owner: Id<Ticket>,
    ) -> Result<Change, AppError>;
    /// Adds the ticket at the end of the list, or links it again at its old place.
    async fn join_worktree(
        &mut self,
        worktree: Id<Worktree>,
        ticket: Id<Ticket>,
        at: Timestamp,
    ) -> Result<Change, AppError>;
    /// Ends the ticket's live link; the ticket stays in the list. `None` when it had none.
    async fn release_link(
        &mut self,
        ticket: Id<Ticket>,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    /// Marks the live worktree with this branch removed and ends its links; `None` without one.
    async fn retire_branch(
        &mut self,
        repository: Id<Repository>,
        runner: Id<Runner>,
        branch: &WorktreeBranch,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    /// The worktree the ticket is linked to right now.
    async fn live_worktree(
        &mut self,
        ticket: Id<Ticket>,
    ) -> Result<Option<WorktreeEntry>, AppError>;
    /// Writes the merge readiness; `None` when it was already so.
    async fn set_ready(
        &mut self,
        worktree: Id<Worktree>,
        ready: bool,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    /// Claims a present worktree for removal (desired absent); `None` when another cleanup has it.
    async fn claim_removal(
        &mut self,
        worktree: Id<Worktree>,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    /// The runner removed the claimed folder: actual removed, no error.
    async fn mark_removed(
        &mut self,
        worktree: Id<Worktree>,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    /// The folder is back on its branch; `None` when the row is claimed or removed, never undone.
    async fn mark_restored(
        &mut self,
        worktree: Id<Worktree>,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    /// The folder is back on its branch for `ensure`, which restores a removed row too.
    async fn mark_ensured(
        &mut self,
        worktree: Id<Worktree>,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    /// Releases the claim (desired present); a reason becomes the last error, a removed row stays.
    async fn mark_kept(
        &mut self,
        worktree: Id<Worktree>,
        reason: Option<&str>,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
}

/// Reads of the registry; links change only with a ticket, through [`WorktreeWrite`].
#[async_trait]
pub trait WorktreeRepository: Send + Sync {
    async fn get(&self, id: Id<Worktree>) -> Result<Option<WorktreeEntry>, AppError>;
    /// The worktree the ticket is linked to right now.
    async fn of_ticket(&self, ticket: Id<Ticket>) -> Result<Option<WorktreeEntry>, AppError>;
    /// Worktrees of the repository on the runner in registry order, oldest first.
    async fn list(
        &self,
        repository: Id<Repository>,
        runner: Id<Runner>,
    ) -> Result<Vec<WorktreeEntry>, AppError>;
    /// Records where the setup commands stand; `None` when it did already.
    async fn set_setup(
        &self,
        worktree: Id<Worktree>,
        state: SetupState,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
}

/// Another live worktree of the repository on that runner has the branch.
#[must_use]
pub fn branch_exists(branch: &WorktreeBranch) -> AppError {
    AppError::invalid(
        "branch-exists",
        format!("a worktree with branch {} exists already", branch.as_str()),
    )
}

/// The ticket is linked to a worktree already.
#[must_use]
pub fn already_started(ticket: Id<Ticket>) -> AppError {
    AppError::invalid(
        "already-started",
        format!("ticket {ticket} has a branch and worktree already"),
    )
}

/// The ticket already shows a branch.
#[must_use]
pub fn already_running(key: &TicketKey, branch: &str) -> AppError {
    AppError::invalid(
        "already-started",
        format!("{key} already runs on branch {branch}"),
    )
}

#[must_use]
pub fn board_repo_mismatch(key: &TicketKey) -> AppError {
    AppError::invalid(
        "board-repo-mismatch",
        format!("the board of {key} does not belong to the repository of this folder"),
    )
}

#[must_use]
pub fn host_not_started(key: &TicketKey) -> AppError {
    AppError::invalid(
        "host-not-started",
        format!("{key} runs in no worktree of this repository"),
    )
}

#[must_use]
pub fn no_commit() -> AppError {
    AppError::invalid(
        "no-commit",
        "create a first commit, a worktree cannot exist without one",
    )
}

/// Git has the branch already.
#[must_use]
pub fn branch_taken(branch: &str) -> AppError {
    AppError::invalid("branch-exists", format!("branch {branch} exists already"))
}

#[must_use]
pub fn worktree_exists(path: &str) -> AppError {
    AppError::invalid(
        "worktree-exists",
        format!("the folder {path} exists already"),
    )
}

/// The ticket has no branch and worktree to resume.
#[must_use]
pub fn ticket_not_started(key: &TicketKey) -> AppError {
    AppError::invalid("ticket-not-started", format!("{key} runs in no worktree"))
}

/// The folder belongs to something else or its leftovers cannot be moved aside.
#[must_use]
pub fn worktree_mismatch(path: &str, branch: &str, reason: &str) -> AppError {
    AppError::invalid(
        "worktree-mismatch",
        format!("the folder {path} is not a worktree of branch {branch}: {reason}"),
    )
}

/// Longest git error output a problem carries; the log keeps all of it.
const GIT_DETAIL_LIMIT: usize = 1000;

/// A git command failed; `detail` is its error output or exit code.
#[must_use]
pub fn git_failed(args: &[&str], detail: &str) -> AppError {
    let command = redact_credentials(&args.join(" "));
    let detail = redact_credentials(detail);
    tracing::debug!(%command, %detail, "git failed");
    AppError::invalid(
        "git-failed",
        format!(
            "git failed (git {}): {}",
            cap_text(&command),
            cap_text(&detail)
        ),
    )
}

/// Replaces the user and password of every `scheme://user:pass@host` with `***`.
fn redact_credentials(text: &str) -> String {
    let mut redacted = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(found) = rest.find("://") {
        let (head, tail) = rest.split_at(found + 3);
        redacted.push_str(head);
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '/' | '"' | '\'' | '<' | '>'))
            .unwrap_or(tail.len());
        rest = match tail[..end].rfind('@') {
            Some(at) => {
                redacted.push_str("***");
                &tail[at..]
            }
            None => tail,
        };
    }
    redacted.push_str(rest);
    redacted
}

fn cap_text(text: &str) -> String {
    match text.char_indices().nth(GIT_DETAIL_LIMIT) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_owned(),
    }
}

#[must_use]
pub fn invalid(error: &WorktreeError) -> AppError {
    let text = match error {
        WorktreeError::Branch(text) | WorktreeError::Path(text) => *text,
    };
    AppError::text(text, "input-invalid", error.to_string())
}

#[must_use]
pub fn not_found(id: Id<Worktree>) -> AppError {
    AppError::not_found(
        "worktree-not-found",
        format!("worktree {id} does not exist"),
    )
}

/// A ticket change through the API may clear branch and worktree, never set them.
#[must_use]
pub fn link_only_clears() -> AppError {
    AppError::invalid(
        "input-invalid",
        "branch and worktree can only be cleared (null) here, never set",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    #[test]
    fn link_errors_are_bad_input_as_in_legacy() {
        let branch = WorktreeBranch::parse("ticket/ARC-0001-a").unwrap();
        let ticket = Id::from_uuid(uuid::Uuid::from_u128(1));
        for (error, code) in [
            (branch_exists(&branch), "branch-exists"),
            (already_started(ticket), "already-started"),
            (link_only_clears(), "input-invalid"),
            (no_commit(), "no-commit"),
            (branch_taken("b"), "branch-exists"),
            (worktree_exists("/w"), "worktree-exists"),
            (worktree_mismatch("/w", "b", "other"), "worktree-mismatch"),
            (
                ticket_not_started(&TicketKey::parse("ARC-1").unwrap()),
                "ticket-not-started",
            ),
            (git_failed(&["status"], "boom"), "git-failed"),
        ] {
            assert_eq!((error.kind(), error.code()), (ErrorKind::Invalid, code));
        }
        assert_eq!(
            git_failed(&["worktree", "add"], "fatal: x").message(),
            "git failed (git worktree add): fatal: x"
        );
        let missing = not_found(Id::from_uuid(uuid::Uuid::from_u128(2)));
        assert_eq!(missing.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn git_failures_hide_credentials_in_urls() {
        let error = git_failed(
            &["fetch", "https://me:hunter2@example.com/repo.git"],
            "fatal: unable to access 'https://me:hunter2@example.com/repo.git/': 403; \
             ssh://deploy@host.example/x and https://ghp_token@github.com/o/r",
        );
        let message = error.message();
        assert!(!message.contains("hunter2"), "{message}");
        assert!(!message.contains("ghp_token"), "{message}");
        assert!(!message.contains("deploy@"), "{message}");
        assert!(
            message.contains("git fetch https://***@example.com/repo.git)"),
            "{message}"
        );
        assert!(
            message.contains("'https://***@example.com/repo.git/'"),
            "{message}"
        );
        assert!(message.contains("https://***@github.com/o/r"), "{message}");
    }

    #[test]
    fn redaction_leaves_text_without_credentials_alone() {
        for text in ["see https://example.com/a@b", "no url, only a@b", "://", ""] {
            assert_eq!(redact_credentials(text), text);
        }
        assert_eq!(
            redact_credentials("https://u:p@ss@host/x"),
            "https://***@host/x"
        );
        assert_eq!(redact_credentials("://@x ://"), "://***@x ://");
    }

    #[test]
    fn git_failures_cap_long_output() {
        let prefix = "git failed (git status): ".len();
        let long = "e".repeat(GIT_DETAIL_LIMIT + 500);
        let message = git_failed(&["status"], &long).message().to_owned();
        assert!(message.ends_with("eee..."), "{message}");
        assert_eq!(message.len(), prefix + GIT_DETAIL_LIMIT + 3);
        let exact = "e".repeat(GIT_DETAIL_LIMIT);
        assert!(!git_failed(&["status"], &exact).message().ends_with("..."));
        let wide = "\u{e9}".repeat(GIT_DETAIL_LIMIT + 1);
        let message = git_failed(&["status"], &wide).message().to_owned();
        assert!(message.ends_with("\u{e9}..."), "{message}");
    }
}
