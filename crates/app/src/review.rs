//! The review before merge: the changes of a ticket branch, its files at the head and its tests.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Actor, BRANCH_MAX, Id, Review, ReviewState, Run, TestRun, TicketRecord, Timestamp, Worktree,
    change_summary,
};

use crate::repositories::RunnerService;
use crate::tickets::TicketService;
use crate::worktrees::{WorktreeEntry, WorktreeRepository};
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

/// Files a review lists at most; a branch with more shows the first ones.
pub const MAX_REVIEW_FILES: usize = 2000;
/// Bytes of one file's patch; a longer patch is cut.
pub const MAX_PATCH_BYTES: usize = 200_000;
/// Bytes of a file shown at the head of the branch; a larger file is cut.
pub const MAX_FILE_BYTES: usize = 512 * 1024;
/// Bytes of a path inside the repository.
pub const MAX_REVIEW_PATH: usize = 4096;

/// A file the branch changed against its target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    /// The earlier path of a renamed or copied file.
    pub old_path: Option<String>,
    /// The first letter of git's status: A, M, D, R, C or T.
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
}

/// The branch against the target, from their merge base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchChanges {
    /// The target branch, or the branch of the main checkout when the target does not exist.
    pub target: String,
    /// The merge base commit.
    pub base: String,
    pub files: Vec<ChangedFile>,
    /// More files changed than [`MAX_REVIEW_FILES`].
    pub truncated: bool,
}

/// The patch of one file, cut to [`MAX_PATCH_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePatch {
    pub patch: String,
    pub truncated: bool,
}

/// A file as it is at the head of the branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchFile {
    pub size: u64,
    pub binary: bool,
    /// The content as text; none for a binary file.
    pub text: Option<String>,
    /// The file is larger than [`MAX_FILE_BYTES`] and `text` holds its start.
    pub truncated: bool,
}

/// Where a runner finds the branch: its main checkout, the branch and the target to compare with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRequest {
    pub root: String,
    pub branch: String,
    pub target: Option<String>,
}

/// A review with the tests the agent ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReview {
    pub review: Review,
    pub tests: Vec<TestRun>,
}

#[async_trait]
pub trait ReviewRepository: Send + Sync {
    /// Keeps the review as the pending one of its worktree; `None` when the run has one already.
    async fn hold(&self, review: &Review, tests: &[TestRun]) -> Result<Option<Change>, AppError>;
    /// Whether the run left a review.
    async fn held(&self, run: Id<Run>) -> Result<bool, AppError>;
    /// Whether the ticket of the run was blocked for the review already.
    async fn blocked(&self, run: Id<Run>) -> Result<bool, AppError>;
    /// Notes that the ticket of the run was blocked for the review at this moment.
    async fn mark_blocked(&self, run: Id<Run>, at: Timestamp) -> Result<(), AppError>;
    /// Decides the pending review of the ticket's own run; `None` when none waits.
    async fn decide(
        &self,
        ticket: &str,
        state: ReviewState,
        by: Id<Actor>,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    /// The state of the newest review of a run for the ticket (by its key).
    async fn state_of(&self, ticket: &str) -> Result<Option<ReviewState>, AppError>;
    /// The newest review of the worktree.
    async fn latest(&self, worktree: Id<Worktree>) -> Result<Option<StoredReview>, AppError>;
}

#[must_use]
pub fn review_branch_invalid(branch: &str) -> AppError {
    AppError::invalid(
        "review-branch-invalid",
        format!("\"{branch}\" is not a branch name a review can use"),
    )
}

#[must_use]
pub fn review_file_unknown(path: &str, branch: &str) -> AppError {
    AppError::invalid(
        "review-file-unknown",
        format!("\"{path}\" is not a file of the changes of branch {branch}"),
    )
}

#[must_use]
pub fn review_not_available(reason: &str) -> AppError {
    AppError::invalid("review-not-available", reason)
}

/// A branch or target name git cannot read as an option or as part of a range.
pub fn check_branch(branch: &str) -> Result<(), AppError> {
    let bad_char = |letter: char| {
        letter.is_control()
            || letter.is_whitespace()
            || matches!(letter, '\\' | ':' | '?' | '*' | '[' | '^' | '~')
    };
    if branch.is_empty()
        || branch.len() > BRANCH_MAX
        || branch.starts_with(['-', '/'])
        || branch.contains("..")
        || branch.contains("@{")
        || branch.chars().any(bad_char)
    {
        return Err(review_branch_invalid(branch));
    }
    Ok(())
}

/// A path inside the repository that cannot leave it: relative, no empty, `.` or `..` segment.
pub fn check_review_path(path: &str, branch: &str) -> Result<(), AppError> {
    let plain = !path.is_empty()
        && path.len() <= MAX_REVIEW_PATH
        && !path.starts_with(['/', ':'])
        && !path
            .chars()
            .any(|letter| letter.is_control() || letter == '\\')
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
    if plain {
        Ok(())
    } else {
        Err(review_file_unknown(path, branch))
    }
}

/// What a person sees of the review of a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewView {
    pub ticket: String,
    pub branch: Option<String>,
    pub target: Option<String>,
    pub base: Option<String>,
    pub files: Vec<ChangedFile>,
    pub files_truncated: bool,
    pub tests: Vec<TestRun>,
    /// When the review was opened, which is when the tests were read.
    pub tests_at: Option<Timestamp>,
    pub run: Option<Id<Run>>,
    pub state: Option<ReviewState>,
    /// False when the changes cannot be shown; `reason` says why.
    pub available: bool,
    pub reason: Option<String>,
}

/// What opening a review came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    New,
    /// The run left a review before.
    Already,
    /// The ticket has no worktree to review.
    NoWorktree,
    /// The ticket has no branch to review.
    NoBranch,
}

/// Where the branch of a ticket is read.
struct Place {
    request: ReviewRequest,
    worktree: WorktreeEntry,
}

/// The ticket's branch as the runner needs it, or why there is none.
enum Found {
    Place(Box<Place>),
    Unavailable(String),
}

pub struct ReviewService {
    tickets: Arc<TicketService>,
    worktrees: Arc<dyn WorktreeRepository>,
    runners: Arc<RunnerService>,
    reviews: Arc<dyn ReviewRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for ReviewService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewService").finish_non_exhaustive()
    }
}

/// What a review is built from.
pub struct ReviewParts {
    pub tickets: Arc<TicketService>,
    pub worktrees: Arc<dyn WorktreeRepository>,
    pub runners: Arc<RunnerService>,
    pub reviews: Arc<dyn ReviewRepository>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub signal: ChangeSignal,
}

/// Whether the failure is git not being able to compare, which a view reports as a reason.
fn is_git_problem(error: &AppError) -> bool {
    matches!(error.code(), "git-failed" | "review-branch-invalid")
}

impl ReviewService {
    #[must_use]
    pub fn new(parts: ReviewParts) -> Self {
        Self {
            tickets: parts.tickets,
            worktrees: parts.worktrees,
            runners: parts.runners,
            reviews: parts.reviews,
            clock: parts.clock,
            ids: parts.ids,
            signal: parts.signal,
        }
    }

    /// The branch, its main checkout and target; or why the ticket has none to read.
    async fn find(&self, ticket: &TicketRecord) -> Result<Found, AppError> {
        let Some(branch) = ticket.branch.clone() else {
            return Ok(Found::Unavailable(String::from("the ticket has no branch")));
        };
        let Some(worktree) = self.worktrees.of_ticket(ticket.id).await? else {
            return Ok(Found::Unavailable(String::from(
                "the ticket has no worktree",
            )));
        };
        let record = worktree.worktree.record();
        let Some(root) = self.runners.root(record.runner, record.repository).await? else {
            return Ok(Found::Unavailable(String::from(
                "the repository has no checkout on its runner",
            )));
        };
        let target = ticket
            .target_branch
            .as_ref()
            .map(|target| target.as_str().to_owned())
            .or_else(|| {
                record
                    .target_branch
                    .as_ref()
                    .map(|target| target.as_str().to_owned())
            });
        Ok(Found::Place(Box::new(Place {
            request: ReviewRequest {
                root,
                branch,
                target,
            },
            worktree,
        })))
    }

    async fn ticket(&self, key: &str) -> Result<TicketRecord, AppError> {
        Ok(self.tickets.get(key).await?.view.ticket.record().clone())
    }

    /// The changes of the ticket's branch and the tests its run left.
    pub async fn view(&self, key: &str) -> Result<ReviewView, AppError> {
        let ticket = self.ticket(key).await?;
        let mut view = ReviewView {
            ticket: ticket.key.to_string(),
            branch: ticket.branch.clone(),
            target: None,
            base: None,
            files: Vec::new(),
            files_truncated: false,
            tests: Vec::new(),
            tests_at: None,
            run: None,
            state: None,
            available: false,
            reason: None,
        };
        let place = match self.find(&ticket).await? {
            Found::Place(place) => place,
            Found::Unavailable(reason) => {
                view.reason = Some(reason);
                return Ok(view);
            }
        };
        let stored = self.reviews.latest(place.worktree.worktree.id()).await?;
        if let Some(stored) = stored {
            let record = stored.review.record();
            view.tests = stored.tests;
            view.tests_at = Some(record.created_at);
            view.run = record.run;
            view.state = Some(record.state);
        }
        match self.changes_of(&place).await {
            Ok(changes) => {
                view.target = Some(changes.target);
                view.base = Some(changes.base);
                view.files = changes.files;
                view.files_truncated = changes.truncated;
                view.available = true;
            }
            Err(error) if is_git_problem(&error) => {
                view.reason = Some(format!(
                    "the changes cannot be computed: {}",
                    error.message()
                ));
            }
            Err(error) => return Err(error),
        }
        Ok(view)
    }

    async fn changes_of(&self, place: &Place) -> Result<BranchChanges, AppError> {
        let request = &place.request;
        check_branch(&request.branch)?;
        if let Some(target) = &request.target {
            check_branch(target)?;
        }
        let runner = place.worktree.worktree.record().runner;
        self.runners.review_changes(runner, request).await
    }

    /// The patch of one file of the changes.
    pub async fn patch(&self, key: &str, path: &str) -> Result<FilePatch, AppError> {
        let ticket = self.ticket(key).await?;
        let place = self.available(&ticket).await?;
        check_branch(&place.request.branch)?;
        if let Some(target) = &place.request.target {
            check_branch(target)?;
        }
        check_review_path(path, &place.request.branch)?;
        let record = place.worktree.worktree.record();
        self.runners
            .review_patch(record.runner, &place.request, path)
            .await
    }

    /// A file as it is at the head of the ticket's branch.
    pub async fn file(&self, key: &str, path: &str) -> Result<BranchFile, AppError> {
        let ticket = self.ticket(key).await?;
        let place = self.available(&ticket).await?;
        check_branch(&place.request.branch)?;
        check_review_path(path, &place.request.branch)?;
        let record = place.worktree.worktree.record();
        self.runners
            .review_file(
                record.runner,
                &place.request.root,
                &place.request.branch,
                path,
            )
            .await
    }

    async fn available(&self, ticket: &TicketRecord) -> Result<Place, AppError> {
        match self.find(ticket).await? {
            Found::Place(place) => Ok(*place),
            Found::Unavailable(reason) => Err(review_not_available(&reason)),
        }
    }

    /// "n files changed (+a/-d)", or why the changes cannot be counted.
    pub(crate) async fn summary(&self, ticket: &TicketRecord) -> Result<String, AppError> {
        let place = match self.find(ticket).await? {
            Found::Place(place) => place,
            Found::Unavailable(reason) => return Ok(format!("changes not computed ({reason})")),
        };
        match self.changes_of(&place).await {
            Ok(changes) => {
                let additions = changes.files.iter().map(|file| file.additions).sum();
                let deletions = changes.files.iter().map(|file| file.deletions).sum();
                Ok(change_summary(changes.files.len(), additions, deletions))
            }
            Err(error) if is_git_problem(&error) => {
                Ok(format!("changes not computed ({})", error.message()))
            }
            Err(error) => Err(error),
        }
    }

    /// Opens the review of the run's branch; a run that has one already keeps it.
    pub(crate) async fn hold(
        &self,
        ticket: &TicketRecord,
        run: Id<Run>,
        tests: &[TestRun],
    ) -> Result<Held, AppError> {
        let Some(branch) = ticket.branch.clone() else {
            return Ok(Held::NoBranch);
        };
        let Some(entry) = self.worktrees.of_ticket(ticket.id).await? else {
            return Ok(Held::NoWorktree);
        };
        let review = Review::pending(
            self.ids.as_ref().next(),
            entry.worktree.id(),
            Some(run),
            branch,
            self.clock.now(),
        );
        let change = self.reviews.hold(&review, tests).await?;
        if let Some(change) = &change {
            self.signal.notify(change);
        }
        Ok(if change.is_some() {
            Held::New
        } else {
            Held::Already
        })
    }

    /// Records the decision of a person on the pending review; false when none waits.
    pub(crate) async fn decide(
        &self,
        ticket: &str,
        state: ReviewState,
        by: Id<Actor>,
    ) -> Result<bool, AppError> {
        let change = self
            .reviews
            .decide(ticket, state, by, self.clock.now())
            .await?;
        if let Some(change) = &change {
            self.signal.notify(change);
        }
        Ok(change.is_some())
    }

    /// The state of the newest review of a run for the ticket (by its key).
    pub(crate) async fn state_of(&self, ticket: &str) -> Result<Option<ReviewState>, AppError> {
        self.reviews.state_of(ticket).await
    }

    /// Whether the run left a review already.
    pub(crate) async fn held(&self, run: Id<Run>) -> Result<bool, AppError> {
        self.reviews.held(run).await
    }

    /// Whether the ticket of the run was blocked for the review already.
    pub(crate) async fn blocked(&self, run: Id<Run>) -> Result<bool, AppError> {
        self.reviews.blocked(run).await
    }

    /// Notes that the ticket of the run is blocked, so a later pass never blocks it again.
    pub(crate) async fn mark_blocked(&self, run: Id<Run>) -> Result<(), AppError> {
        self.reviews.mark_blocked(run, self.clock.now()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_git_would_read_as_options_or_ranges_are_refused() {
        for branch in [
            "", "-x", "/x", "a..b", "a b", "a:b", "a\\b", "a@{1}", "a\nb", "a^",
        ] {
            let error = check_branch(branch).unwrap_err();
            assert_eq!(error.code(), "review-branch-invalid", "{branch:?}");
        }
        for branch in ["main", "ticket/ARC-0001-login", "release/1.2"] {
            assert!(check_branch(branch).is_ok(), "{branch}");
        }
        assert!(check_branch(&"a".repeat(BRANCH_MAX + 1)).is_err());
    }

    #[test]
    fn paths_that_leave_the_repository_are_refused() {
        for path in [
            "",
            "/etc/passwd",
            "../x",
            "a/../x",
            "a//x",
            "./x",
            "a/.",
            "a\\b",
            ":(top)x",
            "a\0b",
        ] {
            let error = check_review_path(path, "b").unwrap_err();
            assert_eq!(error.code(), "review-file-unknown", "{path:?}");
        }
        for path in ["a", "src/lib.rs", "docs/a b.md", ".github/workflows/ci.yml"] {
            assert!(check_review_path(path, "b").is_ok(), "{path}");
        }
        assert!(check_review_path(&"a".repeat(MAX_REVIEW_PATH + 1), "b").is_err());
    }
}
