//! The git side of a merge, run where the repository is: merge, squash, branch delete.

use async_trait::async_trait;
use nullradix_domain::{HoldKind, MergeStrategy};

use crate::AppError;

/// One branch to merge; paths are local to the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRequest {
    /// Main checkout of the repository.
    pub root: String,
    pub branch: String,
    /// The branch to merge into; none merges into what the main checkout has checked out.
    pub target: Option<String>,
    pub message: String,
    pub strategy: MergeStrategy,
}

/// What a merge came to; a conflict is already undone and nothing is half merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    Merged {
        /// The branch merged into.
        into: String,
        /// Short id of the new head of `into`.
        commit: String,
        /// Where the merged branch stood; deleting the branch needs exactly this tip.
        tip: String,
        /// A merge worktree that could not be closed again.
        warning: Option<String>,
    },
    Conflict {
        files: Vec<String>,
    },
    /// Nothing changed; the reason says why git or the checkout did not take the merge.
    Refused {
        reason: String,
    },
    /// Nothing changed; the folder cannot take the merge now but will once it is clean.
    Held {
        kind: HoldKind,
        /// The folder, as in "the main checkout".
        place: String,
        text: String,
    },
}

/// The branch a merge goes into and the commit it stands at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetTip {
    pub into: String,
    pub tip: String,
}

/// A branch whose merge the target holds already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedEarlier {
    pub into: String,
    pub commit: String,
}

/// The merged branch to delete, only while it still stands where it was merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteBranch {
    pub root: String,
    pub branch: String,
    pub tip: String,
    pub into: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchDeletion {
    Deleted,
    /// The branch stays; the reason says why.
    Kept(String),
}

#[async_trait]
pub trait MergeGit: Send + Sync {
    /// Creates a missing target branch locally; says where it came from, if it did.
    async fn ensure_target(&self, root: &str, target: &str) -> Result<Option<String>, AppError>;
    /// The commit the branch points at; none when it does not exist.
    async fn branch_tip(&self, root: &str, branch: &str) -> Result<Option<String>, AppError>;
    /// Whether the branch is in the target already, as an ancestor or by its squash trailer.
    async fn integrated(
        &self,
        root: &str,
        branch: &str,
        target: Option<&str>,
    ) -> Result<bool, AppError>;
    /// Whether the branch is gone but its merge is in the target already.
    async fn merged_earlier(
        &self,
        root: &str,
        branch: &str,
        target: Option<&str>,
    ) -> Result<Option<MergedEarlier>, AppError>;
    /// The target and its commit; without a target the branch the main checkout has checked out.
    async fn target_tip(
        &self,
        root: &str,
        target: Option<&str>,
    ) -> Result<Option<TargetTip>, AppError>;
    /// Whether a merge session left the branch with `ancestor` in it and its folder clean.
    async fn session_resolved(
        &self,
        root: &str,
        ancestor: &str,
        branch: &str,
        folder: &str,
    ) -> Result<bool, AppError>;
    /// Merges where the target is checked out, or in a merge worktree; a conflict is undone.
    async fn merge(&self, request: &MergeRequest) -> Result<MergeOutcome, AppError>;
    /// Deletes the branch when it is merged, still at `tip` and checked out nowhere.
    async fn delete_merged(&self, request: &DeleteBranch) -> Result<BranchDeletion, AppError>;
}
