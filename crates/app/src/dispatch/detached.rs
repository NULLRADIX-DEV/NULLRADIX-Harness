//! Detached worktrees of side runs (research, planning): created at a ref, always removed after.

use async_trait::async_trait;

use crate::AppError;

/// Why a detached worktree exists; the folder name starts with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetachedPurpose {
    Research,
    Plan,
}

impl DetachedPurpose {
    /// The part of the folder name before the subject key.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::Plan => "plan",
        }
    }

    /// The words for messages.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Research => "research worktree",
            Self::Plan => "plan worktree",
        }
    }

    /// The code of the refusal when the folder is in the way.
    #[must_use]
    pub const fn busy_code(self) -> &'static str {
        match self {
            Self::Research => "research-worktree-busy",
            Self::Plan => "plan-worktree-busy",
        }
    }
}

/// The folder of the worktree: `<repo>-worktrees/<purpose>-<subject>` beside the checkout.
#[must_use]
pub fn detached_path(root: &str, purpose: DetachedPurpose, subject: &str) -> String {
    let text = root.replace('\\', "/");
    let trimmed = text.trim_end_matches('/');
    let (parent, name) = match trimmed.rsplit_once('/') {
        Some(("", name)) => ("", name),
        Some((parent, name)) => (parent, name),
        None => (".", trimmed),
    };
    format!("{parent}/{name}-worktrees/{}-{subject}", purpose.prefix())
}

/// The subject key in the folder name, when it is one the harness could have created.
#[must_use]
pub fn detached_subject(purpose: DetachedPurpose, path: &str) -> Option<String> {
    let text = path.replace('\\', "/");
    let name = text.trim_end_matches('/').rsplit('/').next()?;
    let subject = name.strip_prefix(purpose.prefix())?.strip_prefix('-')?;
    let plain = !subject.is_empty()
        && subject.len() <= 64
        && subject
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    plain.then(|| subject.to_owned())
}

/// The checkout a worktree folder belongs to, with the folder in forward slashes.
#[must_use]
pub fn detached_root(purpose: DetachedPurpose, path: &str) -> Option<(String, String)> {
    detached_subject(purpose, path)?;
    let text = path.replace('\\', "/");
    let (parent, _) = text.trim_end_matches('/').rsplit_once('/')?;
    let (outer, folder) = parent.rsplit_once('/').unwrap_or((".", parent));
    let name = folder
        .strip_suffix("-worktrees")
        .filter(|name| !name.is_empty())?;
    let root = format!("{outer}/{name}");
    Some((root, text.trim_end_matches('/').to_owned()))
}

/// What the runner needs to add a detached worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachedCreate {
    /// Main checkout, where git runs.
    pub root: String,
    pub purpose: DetachedPurpose,
    /// Question or ticket key the folder is named after.
    pub subject: String,
    /// The commit or branch the folder starts at.
    pub at: String,
}

/// The runner's answer to a create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetachedCreated {
    Created {
        path: String,
        /// Whether `core.longpaths` was just switched on in the repository.
        long_paths_enabled: bool,
    },
    /// The folder exists and could not be removed; it stays untouched.
    Busy { path: String },
}

/// What the runner needs to remove one detached worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachedRemove {
    pub root: String,
    pub purpose: DetachedPurpose,
    pub path: String,
}

/// The runner's answer to a removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetachedRemoved {
    Removed,
    /// The folder stays; the reason says why.
    Kept(String),
}

/// Git work for detached worktrees; only folders at their expected place are ever removed.
#[async_trait]
pub trait DetachedWorktreePort: Send + Sync {
    /// Adds the folder at `at`, first removing a rest of an earlier run.
    async fn create(&self, request: &DetachedCreate) -> Result<DetachedCreated, AppError>;
    /// Removes the folder with its links and git's entry; a folder gone already counts as removed.
    async fn remove(&self, request: &DetachedRemove) -> Result<DetachedRemoved, AppError>;
    /// The folders git lists for the checkout that sit at the expected place of `purpose`.
    async fn list(&self, root: &str, purpose: DetachedPurpose) -> Result<Vec<String>, AppError>;
}

/// The refusal for a folder that cannot be made free.
#[must_use]
pub fn detached_busy(purpose: DetachedPurpose, path: &str) -> AppError {
    AppError::conflict(
        purpose.busy_code(),
        format!("the folder {path} exists and is not a removable worktree - check it by hand"),
    )
}

/// The ref a worktree starts at must be a plain name or hash, never an option.
#[must_use]
pub fn detached_ref_invalid(at: &str) -> AppError {
    AppError::invalid(
        "git-ref-invalid",
        format!("\"{at}\" is not a ref a worktree can start at"),
    )
}

/// Whether `at` may be handed to git as the start of a worktree.
#[must_use]
pub fn detached_ref_valid(at: &str) -> bool {
    !at.is_empty() && !at.starts_with('-') && !at.chars().any(|c| c.is_control() || c == ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ref_may_not_look_like_an_option() {
        assert!(detached_ref_valid("HEAD"));
        assert!(detached_ref_valid("refs/heads/main"));
        assert!(!detached_ref_valid("--output=x"));
        assert!(!detached_ref_valid(""));
        assert!(!detached_ref_valid("a b"));
    }

    #[test]
    fn the_folder_lies_beside_the_checkout() {
        assert_eq!(
            detached_path("C:\\work\\arches\\", DetachedPurpose::Research, "ARC-Q7"),
            "C:/work/arches-worktrees/research-ARC-Q7"
        );
        assert_eq!(
            detached_path("/arches", DetachedPurpose::Plan, "ARC-7"),
            "/arches-worktrees/plan-ARC-7"
        );
        assert_eq!(
            detached_path("arches", DetachedPurpose::Research, "ARC-Q1"),
            "./arches-worktrees/research-ARC-Q1"
        );
    }

    #[test]
    fn the_checkout_is_found_again_from_the_folder() {
        let root = |path| detached_root(DetachedPurpose::Research, path);
        assert_eq!(
            root(r"C:\work\arches-worktrees\research-ARC-Q7"),
            Some((
                "C:/work/arches".into(),
                "C:/work/arches-worktrees/research-ARC-Q7".into()
            ))
        );
        assert_eq!(
            root("/arches-worktrees/research-ARC-Q7").map(|found| found.0),
            Some("/arches".into())
        );
        assert_eq!(root("/w/arches/research-ARC-Q7"), None);
        assert_eq!(root("/w/-worktrees/research-ARC-Q7"), None);
        assert_eq!(root("/w/arches-worktrees/plan-ARC-7"), None);
        let path = detached_path("/w/arches", DetachedPurpose::Research, "ARC-Q7");
        assert_eq!(root(&path).map(|found| found.0), Some("/w/arches".into()));
    }

    #[test]
    fn only_a_plain_subject_after_the_prefix_is_ours() {
        let own = |path| detached_subject(DetachedPurpose::Research, path);
        assert_eq!(own("/w/a-worktrees/research-ARC-Q7"), Some("ARC-Q7".into()));
        assert_eq!(
            own("/w/a-worktrees/research-ARC-Q7/"),
            Some("ARC-Q7".into())
        );
        assert_eq!(own("/w/a-worktrees/plan-ARC-7"), None);
        assert_eq!(own("/w/a-worktrees/research-"), None);
        assert_eq!(own("/w/a-worktrees/research-a b"), None);
        assert_eq!(own("/w/a-worktrees/research-.."), None);
        assert_eq!(own("/w/a-worktrees/researchARC"), None);
        let long = format!("/w/a-worktrees/research-{}", "x".repeat(65));
        assert_eq!(own(&long), None);
    }
}
