//! Adding a ticket's worktree again on its branch, as legacy `ensure_worktree` did.

use std::fs;
use std::path::{Path, PathBuf};

use nullradix_app::AppError;
use nullradix_app::worktrees::{EnsureWorktree, EnsuredWorktree, worktree_mismatch};

use crate::cleanup::{blocking, listed};
use crate::git::local_key;
use crate::worktree::{LONG_PATHS, ensure_long_paths, exists, git, has_ref};

/// Brings the folder back as a worktree; leftovers move aside, only an empty folder is deleted.
pub(crate) async fn ensure(
    request: &EnsureWorktree,
    plugin: Option<&str>,
) -> Result<EnsuredWorktree, AppError> {
    let root = Path::new(&request.root);
    if !has_ref(root, &format!("refs/heads/{}", request.branch)).await? {
        return Ok(EnsuredWorktree::BranchMissing);
    }
    let path = PathBuf::from(&request.path);
    let mut moved_aside = None;
    if exists(path.clone()).await? {
        if holds(root, &path, &request.branch).await? {
            crate::plugin::enable(&path, plugin).await;
            return Ok(EnsuredWorktree::Present);
        }
        moved_aside = clear_rest(root, request).await?;
    }
    git(root, &["worktree", "prune"]).await?;
    ensure_long_paths(root, cfg!(windows)).await?;
    let mut args: Vec<&str> = LONG_PATHS.to_vec();
    args.extend(["worktree", "add", "--", &request.path, &request.branch]);
    git(root, &args).await?;
    crate::plugin::enable(&path, plugin).await;
    Ok(EnsuredWorktree::Restored { moved_aside })
}

/// Whether git lists the folder as a worktree with this branch checked out.
async fn holds(root: &Path, path: &Path, branch: &str) -> Result<bool, AppError> {
    let list = git(root, &["worktree", "list", "--porcelain"]).await?;
    let (path, branch) = (path.to_path_buf(), branch.to_owned());
    blocking(move || lists_branch(&list, &path, &branch)).await
}

fn lists_branch(list: &str, path: &Path, branch: &str) -> bool {
    let wanted = local_key(path);
    let head = format!("branch refs/heads/{branch}");
    let mut current: Option<String> = None;
    for line in list.split('\n').map(str::trim) {
        if let Some(found) = line.strip_prefix("worktree ") {
            current = Some(local_key(Path::new(found)));
        } else if line == head {
            if current.as_deref() == Some(wanted.as_str()) {
                return true;
            }
        } else if line.is_empty() {
            current = None;
        }
    }
    false
}

/// Frees the place of a half-removed worktree; the app keeps cleanup away meanwhile.
async fn clear_rest(root: &Path, request: &EnsureWorktree) -> Result<Option<String>, AppError> {
    let path = PathBuf::from(&request.path);
    let unlisted = listed(root, &request.path).await == Some(false);
    let folder = path.clone();
    let rest = blocking(move || unlisted && !folder.join(".git").exists()).await?;
    if !rest {
        return Err(worktree_mismatch(
            &request.path,
            &request.branch,
            "it belongs to something else; check it and clean up by hand",
        ));
    }
    let moved = blocking(move || set_aside(&path)).await?;
    moved.map_err(|error| {
        worktree_mismatch(
            &request.path,
            &request.branch,
            &format!(
                "the leftovers of a removed worktree could not be moved aside ({error}); \
                 a program may hold files open, close it or delete the folder by hand"
            ),
        )
    })
}

/// An empty folder is deleted, anything else moves to the first free `<folder>.rest[-n]`.
fn set_aside(path: &Path) -> std::io::Result<Option<String>> {
    if fs::read_dir(path)?.next().is_none() {
        fs::remove_dir(path)?;
        return Ok(None);
    }
    let rest = free_rest(path);
    fs::rename(path, &rest)?;
    Ok(Some(rest.to_string_lossy().replace('\\', "/")))
}

fn free_rest(path: &Path) -> PathBuf {
    let named = |suffix: String| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let first = named(String::from(".rest"));
    if !first.exists() {
        return first;
    }
    (2..=u32::MAX)
        .map(|number| named(format!(".rest-{number}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;

    fn git_sync(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn text(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    /// A repository with the worktree `ARC-0001` on `ticket/a`.
    fn setup(dir: &Path) -> (PathBuf, PathBuf) {
        let root = dir.join("arches");
        fs::create_dir_all(&root).unwrap();
        git_sync(&root, &["init", "-q", "-b", "main"]);
        git_sync(&root, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let path = dir.join("arches-worktrees").join("ARC-0001");
        git_sync(
            &root,
            &["worktree", "add", "-q", "-b", "ticket/a", &text(&path)],
        );
        (root, path)
    }

    fn request(root: &Path, path: &Path) -> EnsureWorktree {
        EnsureWorktree {
            root: text(root),
            branch: String::from("ticket/a"),
            path: text(path),
        }
    }

    fn mismatch(result: Result<EnsuredWorktree, AppError>) -> String {
        let error = result.unwrap_err();
        assert_eq!(error.code(), "worktree-mismatch");
        error.message().to_owned()
    }

    #[tokio::test]
    async fn a_resumed_and_a_restored_worktree_have_the_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        assert_eq!(
            ensure(&request(&root, &path), Some(crate::plugin::tests::ID))
                .await
                .unwrap(),
            EnsuredWorktree::Present
        );
        crate::plugin::tests::assert_enabled(&path);
        git_sync(&root, &["worktree", "remove", "--force", &text(&path)]);
        assert!(matches!(
            ensure(&request(&root, &path), Some(crate::plugin::tests::ID))
                .await
                .unwrap(),
            EnsuredWorktree::Restored { .. }
        ));
        crate::plugin::tests::assert_enabled(&path);
    }

    #[tokio::test]
    async fn a_listed_worktree_of_the_branch_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        fs::write(path.join("work.txt"), "unsaved").unwrap();
        let ensured = ensure(&request(&root, &path), None).await.unwrap();
        assert_eq!(ensured, EnsuredWorktree::Present);
        assert!(path.join("work.txt").is_file());
    }

    #[tokio::test]
    async fn a_removed_folder_comes_back_on_the_same_branch() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        let tip = git_sync(&root, &["rev-parse", "refs/heads/ticket/a"]);
        fs::remove_dir_all(&path).unwrap();
        let ensured = ensure(&request(&root, &path), None).await.unwrap();
        assert_eq!(ensured, EnsuredWorktree::Restored { moved_aside: None });
        assert_eq!(git_sync(&path, &["branch", "--show-current"]), "ticket/a");
        assert_eq!(git_sync(&path, &["rev-parse", "HEAD"]), tip);
        let again = ensure(&request(&root, &path), None).await.unwrap();
        assert_eq!(again, EnsuredWorktree::Present);
    }

    #[tokio::test]
    async fn a_worktree_removed_through_git_comes_back_too() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        git_sync(&root, &["worktree", "remove", "--force", &text(&path)]);
        let ensured = ensure(&request(&root, &path), None).await.unwrap();
        assert_eq!(ensured, EnsuredWorktree::Restored { moved_aside: None });
        assert!(path.join(".git").exists());
    }

    #[tokio::test]
    async fn an_empty_leftover_folder_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        git_sync(&root, &["worktree", "remove", "--force", &text(&path)]);
        fs::create_dir_all(&path).unwrap();
        let ensured = ensure(&request(&root, &path), None).await.unwrap();
        assert_eq!(ensured, EnsuredWorktree::Restored { moved_aside: None });
        assert!(path.join(".git").exists());
    }

    #[tokio::test]
    async fn a_leftover_with_files_moves_aside_under_the_first_free_name() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        for round in ["rest", "rest-2"] {
            git_sync(&root, &["worktree", "remove", "--force", &text(&path)]);
            fs::create_dir_all(path.join("node_modules")).unwrap();
            fs::write(path.join("node_modules/locked.bin"), round).unwrap();
            let ensured = ensure(&request(&root, &path), None).await.unwrap();
            let aside = format!("{}.{round}", text(&path));
            assert_eq!(
                ensured,
                EnsuredWorktree::Restored {
                    moved_aside: Some(aside.clone())
                }
            );
            let kept = fs::read_to_string(Path::new(&aside).join("node_modules/locked.bin"));
            assert_eq!(kept.unwrap(), round);
        }
    }

    #[tokio::test]
    async fn a_folder_of_something_else_is_a_mismatch_and_stays_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        git_sync(&root, &["worktree", "remove", "--force", &text(&path)]);
        // a repository of its own in the place
        fs::create_dir_all(&path).unwrap();
        git_sync(&path, &["init", "-q"]);
        let message = mismatch(ensure(&request(&root, &path), None).await);
        assert!(message.contains("belongs to something else"), "{message}");
        assert!(path.join(".git").is_dir());
        assert!(!Path::new(&format!("{}.rest", text(&path))).exists());
        // the worktree of another branch in the place
        fs::remove_dir_all(&path).unwrap();
        git_sync(&root, &["worktree", "prune"]);
        git_sync(
            &root,
            &["worktree", "add", "-q", "-b", "other", &text(&path)],
        );
        mismatch(ensure(&request(&root, &path), None).await);
        assert_eq!(git_sync(&path, &["branch", "--show-current"]), "other");
    }

    #[tokio::test]
    async fn a_missing_branch_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        git_sync(&root, &["worktree", "remove", "--force", &text(&path)]);
        git_sync(&root, &["branch", "-D", "ticket/a"]);
        let ensured = ensure(&request(&root, &path), None).await.unwrap();
        assert_eq!(ensured, EnsuredWorktree::BranchMissing);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_branch_checked_out_elsewhere_is_a_git_failure() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        let elsewhere = dir.path().join("elsewhere");
        fs::rename(&path, &elsewhere).unwrap();
        git_sync(&root, &["worktree", "repair", &text(&elsewhere)]);
        let error = ensure(&request(&root, &path), None).await.unwrap_err();
        assert_eq!(error.code(), "git-failed");
    }

    #[test]
    fn paths_compare_with_the_case_rule_of_the_system() {
        let list = "worktree C:/Work/Arches\nbranch refs/heads/main\n\n\
                    worktree C:/Work/Arches-worktrees/ARC-0001\nbranch refs/heads/ticket/a\n";
        let other_case = Path::new("c:/work/arches-worktrees/arc-0001");
        let ignores = cfg!(any(windows, target_os = "macos"));
        assert_eq!(lists_branch(list, other_case, "ticket/a"), ignores);
        let exact = Path::new("C:/Work/Arches-worktrees/ARC-0001");
        assert!(lists_branch(list, exact, "ticket/a"));
        assert!(!lists_branch(list, exact, "main"));
    }
}
