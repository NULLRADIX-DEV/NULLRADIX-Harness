//! Detached worktrees of research and plan runs; removed only at their expected place.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::dispatch::detached::{
    DetachedCreate, DetachedCreated, DetachedPurpose, DetachedRemove, DetachedRemoved,
    DetachedWorktreePort, detached_path, detached_ref_invalid, detached_ref_valid,
    detached_subject,
};

use crate::cleanup::{blocking, remove_links};
use crate::git::local_key;
use crate::worktree::{LONG_PATHS, attempt, ensure_long_paths, exists, git};

/// Runs git on this machine for detached worktrees.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalDetached;

/// Whether `path` is exactly where the worktree of its subject lies beside `root`.
async fn own_place(root: &str, purpose: DetachedPurpose, path: &str) -> Result<bool, AppError> {
    let Some(subject) = detached_subject(purpose, path) else {
        return Ok(false);
    };
    let expected = detached_path(root, purpose, &subject);
    let path = path.to_owned();
    blocking(move || local_key(Path::new(&path)) == local_key(Path::new(&expected))).await
}

async fn remove(request: &DetachedRemove) -> Result<DetachedRemoved, AppError> {
    let label = request.purpose.label();
    if !own_place(&request.root, request.purpose, &request.path).await? {
        return Ok(DetachedRemoved::Kept(format!(
            "{} is not a {label} - not removed",
            request.path
        )));
    }
    let folder = PathBuf::from(&request.path);
    let present = exists(folder.clone()).await?;
    if present {
        // links go first, as git could follow a junction and delete what it points to
        let links = blocking(move || remove_links(&folder)).await?;
        if !links.is_empty() {
            let names: Vec<String> = links
                .iter()
                .map(|link| link.to_string_lossy().into_owned())
                .collect();
            return Ok(DetachedRemoved::Kept(format!(
                "{label} {} not removed: its links could not be removed ({})",
                request.path,
                names.join(", ")
            )));
        }
    }
    let root = Path::new(&request.root);
    let removed = git(root, &["worktree", "remove", "--force", &request.path]).await;
    // a stale entry of a folder that is gone must not stay in git's list
    let _ = attempt(root, &["worktree", "prune"]).await?;
    match removed {
        Ok(_) => Ok(DetachedRemoved::Removed),
        Err(error) => {
            if exists(PathBuf::from(&request.path)).await? {
                Ok(DetachedRemoved::Kept(format!(
                    "{label} {} not removed: {}",
                    request.path,
                    error.message()
                )))
            } else {
                Ok(DetachedRemoved::Removed)
            }
        }
    }
}

#[async_trait]
impl DetachedWorktreePort for LocalDetached {
    async fn create(&self, request: &DetachedCreate) -> Result<DetachedCreated, AppError> {
        if !detached_ref_valid(&request.at) {
            return Err(detached_ref_invalid(&request.at));
        }
        let path = detached_path(&request.root, request.purpose, &request.subject);
        if exists(PathBuf::from(&path)).await? {
            let cleared = remove(&DetachedRemove {
                root: request.root.clone(),
                purpose: request.purpose,
                path: path.clone(),
            })
            .await?;
            if matches!(cleared, DetachedRemoved::Kept(_)) || exists(PathBuf::from(&path)).await? {
                return Ok(DetachedCreated::Busy { path });
            }
        }
        let root = Path::new(&request.root);
        let long_paths_enabled = ensure_long_paths(root, cfg!(windows)).await?;
        let mut args: Vec<&str> = LONG_PATHS.to_vec();
        args.extend(["worktree", "add", "--detach", &path, &request.at]);
        git(root, &args).await?;
        Ok(DetachedCreated::Created {
            path,
            long_paths_enabled,
        })
    }

    async fn remove(&self, request: &DetachedRemove) -> Result<DetachedRemoved, AppError> {
        remove(request).await
    }

    async fn list(&self, root: &str, purpose: DetachedPurpose) -> Result<Vec<String>, AppError> {
        let Some(list) = attempt(Path::new(root), &["worktree", "list", "--porcelain"]).await?
        else {
            return Ok(Vec::new());
        };
        let mut found = Vec::new();
        for path in list
            .lines()
            .filter_map(|line| line.trim().strip_prefix("worktree "))
        {
            let path = path.trim();
            if own_place(root, purpose, path).await?
                && let Some(subject) = detached_subject(purpose, path)
            {
                // our own spelling, as the runs carry it; git writes the real path of a link
                found.push(detached_path(root, purpose, &subject));
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
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

    fn keys(paths: &[String]) -> Vec<String> {
        paths
            .iter()
            .map(|path| local_key(Path::new(path)))
            .collect()
    }

    fn repository(dir: &Path) -> PathBuf {
        let root = dir.join("arches");
        fs::create_dir_all(&root).unwrap();
        git_sync(&root, &["init", "-q", "-b", "main"]);
        git_sync(&root, &["commit", "-q", "--allow-empty", "-m", "one"]);
        root
    }

    fn create(root: &Path, subject: &str, at: &str) -> DetachedCreate {
        DetachedCreate {
            root: text(root),
            purpose: DetachedPurpose::Research,
            subject: subject.to_owned(),
            at: at.to_owned(),
        }
    }

    fn created_path(created: DetachedCreated) -> String {
        match created {
            DetachedCreated::Created { path, .. } => path,
            DetachedCreated::Busy { path } => panic!("busy: {path}"),
        }
    }

    #[tokio::test]
    async fn adds_a_detached_folder_beside_the_checkout_and_removes_only_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        let first = git_sync(&root, &["rev-parse", "HEAD"]);
        git_sync(&root, &["commit", "-q", "--allow-empty", "-m", "two"]);
        let created = LocalDetached
            .create(&create(&root, "ARC-Q0007", &first))
            .await
            .unwrap();
        let path = created_path(created);
        assert_eq!(
            path,
            text(
                &dir.path()
                    .join("arches-worktrees")
                    .join("research-ARC-Q0007")
            )
        );
        let folder = PathBuf::from(&path);
        assert_eq!(git_sync(&folder, &["rev-parse", "HEAD"]), first);
        assert_eq!(git_sync(&folder, &["branch", "--show-current"]), "");
        assert!(!git_sync(&root, &["branch", "--list"]).contains("research"));
        fs::write(folder.join("research.json"), "{}").unwrap();
        let listed = LocalDetached
            .list(&text(&root), DetachedPurpose::Research)
            .await
            .unwrap();
        assert_eq!(keys(&listed), keys(std::slice::from_ref(&path)));
        let removed = LocalDetached
            .remove(&DetachedRemove {
                root: text(&root),
                purpose: DetachedPurpose::Research,
                path: path.clone(),
            })
            .await
            .unwrap();
        assert_eq!(removed, DetachedRemoved::Removed);
        assert!(!folder.exists());
        assert!(!git_sync(&root, &["worktree", "list"]).contains("research-ARC-Q0007"));
        assert!(root.exists());
    }

    #[tokio::test]
    async fn a_rest_of_an_earlier_run_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        let first = created_path(
            LocalDetached
                .create(&create(&root, "ARC-Q0001", "HEAD"))
                .await
                .unwrap(),
        );
        fs::write(Path::new(&first).join("research.json"), "old").unwrap();
        let second = created_path(
            LocalDetached
                .create(&create(&root, "ARC-Q0001", "HEAD"))
                .await
                .unwrap(),
        );
        assert_eq!(second, first);
        assert!(!Path::new(&second).join("research.json").exists());
    }

    #[tokio::test]
    async fn a_folder_that_is_not_a_worktree_stays_and_is_reported_busy() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        let place = dir
            .path()
            .join("arches-worktrees")
            .join("research-ARC-Q0002");
        fs::create_dir_all(&place).unwrap();
        fs::write(place.join("mine.txt"), "keep").unwrap();
        let created = LocalDetached
            .create(&create(&root, "ARC-Q0002", "HEAD"))
            .await
            .unwrap();
        assert!(
            matches!(created, DetachedCreated::Busy { .. }),
            "{created:?}"
        );
        assert!(place.join("mine.txt").exists());
    }

    #[tokio::test]
    async fn folders_outside_the_expected_place_are_never_removed() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        let remove = |path: String| DetachedRemove {
            root: text(&root),
            purpose: DetachedPurpose::Research,
            path,
        };
        let other = dir.path().join("other");
        let elsewhere = dir.path().join("research-ARC-Q0003");
        let wrong_prefix = dir.path().join("arches-worktrees").join("plan-ARC-7");
        for folder in [&other, &elsewhere, &wrong_prefix] {
            fs::create_dir_all(folder).unwrap();
            let kept = LocalDetached.remove(&remove(text(folder))).await.unwrap();
            assert!(matches!(kept, DetachedRemoved::Kept(_)), "{folder:?}");
            assert!(folder.exists());
        }
        let main = LocalDetached.remove(&remove(text(&root))).await.unwrap();
        assert!(matches!(main, DetachedRemoved::Kept(_)));
        assert!(root.join(".git").exists());
    }

    #[tokio::test]
    async fn a_missing_folder_counts_as_removed_and_its_entry_is_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        let path = created_path(
            LocalDetached
                .create(&create(&root, "ARC-Q0004", "HEAD"))
                .await
                .unwrap(),
        );
        fs::remove_dir_all(&path).unwrap();
        let removed = LocalDetached
            .remove(&DetachedRemove {
                root: text(&root),
                purpose: DetachedPurpose::Research,
                path,
            })
            .await
            .unwrap();
        assert_eq!(removed, DetachedRemoved::Removed);
        assert!(!git_sync(&root, &["worktree", "list"]).contains("research-ARC-Q0004"));
    }

    #[tokio::test]
    async fn a_ref_that_looks_like_an_option_is_refused_before_git_runs() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        let error = LocalDetached
            .create(&create(&root, "ARC-Q0005", "--orphan"))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "git-ref-invalid");
        assert!(!dir.path().join("arches-worktrees").exists());
    }

    #[tokio::test]
    async fn a_ref_that_does_not_exist_fails_and_leaves_no_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        let error = LocalDetached
            .create(&create(&root, "ARC-Q0006", "nope"))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "git-failed");
        assert!(
            !dir.path()
                .join("arches-worktrees")
                .join("research-ARC-Q0006")
                .exists()
        );
    }

    #[tokio::test]
    async fn plan_and_research_folders_are_listed_apart() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        let research = created_path(
            LocalDetached
                .create(&create(&root, "ARC-Q0008", "HEAD"))
                .await
                .unwrap(),
        );
        let plan = created_path(
            LocalDetached
                .create(&DetachedCreate {
                    purpose: DetachedPurpose::Plan,
                    ..create(&root, "ARC-0008", "HEAD")
                })
                .await
                .unwrap(),
        );
        let research_list = LocalDetached
            .list(&text(&root), DetachedPurpose::Research)
            .await
            .unwrap();
        let plan_list = LocalDetached
            .list(&text(&root), DetachedPurpose::Plan)
            .await
            .unwrap();
        assert_eq!(keys(&research_list), keys(&[research]));
        assert_eq!(keys(&plan_list), keys(&[plan]));
    }

    /// A junction on Windows, which needs no privileges; a symlink elsewhere.
    fn link_dir(target: &Path, link: &Path) {
        #[cfg(windows)]
        {
            let output = Command::new("cmd")
                .arg("/C")
                .arg("mklink")
                .arg("/J")
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(output.status.success(), "mklink /J");
        }
        #[cfg(not(windows))]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[tokio::test]
    async fn a_root_behind_a_link_lists_the_folders_in_the_spelling_of_the_runs() {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
        let real = base.join("real");
        fs::create_dir_all(&real).unwrap();
        repository(&real);
        let linked = base.join("linked");
        link_dir(&real, &linked);
        created_path(
            LocalDetached
                .create(&create(&real.join("arches"), "ARC-Q0009", "HEAD"))
                .await
                .unwrap(),
        );
        let root = linked.join("arches");
        let path = detached_path(&text(&root), DetachedPurpose::Research, "ARC-Q0009");
        let listed = LocalDetached
            .list(&text(&root), DetachedPurpose::Research)
            .await
            .unwrap();
        assert_eq!(listed, vec![path]);
    }
}
