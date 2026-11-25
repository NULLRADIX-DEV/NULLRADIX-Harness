//! Git steps of a ticket start: checks, target branch, long paths and `worktree add`.

use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use nullradix_app::AppError;
use nullradix_app::worktrees::{
    CreatedWorktree, NewWorktree, branch_taken, git_failed, no_commit, worktree_exists,
};

/// Allows long paths for this call even when the repository config could not be set.
const LONG_PATHS: [&str; 2] = ["-c", "core.longpaths=true"];

/// Runs git with argv and no shell, without a timeout as in legacy.
async fn output(cwd: &Path, args: &[&str]) -> Result<Output, AppError> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .output()
        .await
        .map_err(|error| git_failed(args, &error.to_string()))
}

/// Trimmed standard output, or `git-failed` with the error output or exit code.
async fn git(cwd: &Path, args: &[&str]) -> Result<String, AppError> {
    let output = output(cwd, args).await?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let detail = if !stderr.is_empty() {
        stderr
    } else if let Some(code) = output.status.code() {
        format!("git {} exited with code {code}", args.join(" "))
    } else {
        format!("git {} was terminated", args.join(" "))
    };
    Err(git_failed(args, &detail))
}

/// Trimmed output when git succeeds, `None` when it fails.
async fn attempt(cwd: &Path, args: &[&str]) -> Result<Option<String>, AppError> {
    let output = output(cwd, args).await?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

async fn has_ref(cwd: &Path, reference: &str) -> Result<bool, AppError> {
    Ok(
        attempt(cwd, &["rev-parse", "--verify", "--quiet", reference])
            .await?
            .is_some(),
    )
}

async fn exists(path: PathBuf) -> Result<bool, AppError> {
    tokio::task::spawn_blocking(move || path.exists())
        .await
        .map_err(AppError::internal)
}

/// Creates the worktree after the legacy checks, in the legacy order.
pub(crate) async fn create(request: &NewWorktree) -> Result<CreatedWorktree, AppError> {
    let cwd = Path::new(&request.cwd);
    if attempt(cwd, &["rev-parse", "--verify", "HEAD"])
        .await?
        .is_none()
    {
        return Err(no_commit());
    }
    if has_ref(cwd, &format!("refs/heads/{}", request.branch)).await? {
        return Err(branch_taken(&request.branch));
    }
    if exists(PathBuf::from(&request.path)).await? {
        return Err(worktree_exists(&request.path));
    }
    let target_created_from = match &request.target {
        Some(target) => ensure_target(Path::new(&request.root), target).await?,
        None => None,
    };
    let long_paths_enabled = ensure_long_paths(cwd, cfg!(windows)).await?;
    let start = request
        .target
        .as_ref()
        .map(|target| format!("refs/heads/{target}"));
    let mut args: Vec<&str> = LONG_PATHS.to_vec();
    args.extend(["worktree", "add", "-b", &request.branch, &request.path]);
    if let Some(start) = &start {
        args.push(start);
    }
    git(cwd, &args).await?;
    Ok(CreatedWorktree {
        target_created_from,
        long_paths_enabled,
    })
}

/// Creates a missing target branch locally and says from where; never fetches or pushes.
async fn ensure_target(root: &Path, name: &str) -> Result<Option<String>, AppError> {
    if has_ref(root, &format!("refs/heads/{name}")).await? {
        return Ok(None);
    }
    let remote = format!("refs/remotes/origin/{name}");
    if has_ref(root, &remote).await? {
        git(root, &["branch", "--no-track", name, &remote]).await?;
        return Ok(Some(format!("origin/{name}")));
    }
    let current = git(root, &["branch", "--show-current"]).await?;
    let from = if current.is_empty() {
        git(root, &["rev-parse", "--short", "HEAD"]).await?
    } else {
        current
    };
    git(root, &["branch", "--no-track", name, "HEAD"]).await?;
    Ok(Some(from))
}

/// Sets `core.longpaths` on Windows unless it is on; true when it was set just now.
async fn ensure_long_paths(cwd: &Path, windows: bool) -> Result<bool, AppError> {
    if !windows {
        return Ok(false);
    }
    let current = attempt(cwd, &["config", "--get", "core.longpaths"]).await?;
    if current.is_some_and(|value| value.eq_ignore_ascii_case("true")) {
        return Ok(false);
    }
    Ok(attempt(cwd, &["config", "core.longpaths", "true"])
        .await?
        .is_some())
}

/// Whether each folder exists, in the order given.
pub(crate) async fn folders_exist(paths: &[String]) -> Result<Vec<bool>, AppError> {
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    tokio::task::spawn_blocking(move || paths.iter().map(|path| path.is_dir()).collect())
        .await
        .map_err(AppError::internal)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;

    fn git_sync(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "protocol.file.allow=always"])
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn repository(path: &Path) {
        fs::create_dir_all(path).unwrap();
        git_sync(path, &["init", "-q", "-b", "main"]);
        git_sync(path, &["commit", "-q", "--allow-empty", "-m", "init"]);
    }

    fn text(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    fn request(root: &Path, branch: &str, target: Option<&str>) -> NewWorktree {
        let parent = root.parent().unwrap();
        NewWorktree {
            cwd: text(root),
            root: text(root),
            branch: branch.to_owned(),
            path: text(&parent.join("arches-worktrees").join("ARC-0001")),
            target: target.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn adds_a_worktree_on_a_new_branch_from_head() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("arches");
        repository(&root);
        let wanted = request(&root, "ticket/ARC-0001-a", None);
        let created = create(&wanted).await.unwrap();
        assert_eq!(created.target_created_from, None);
        assert_eq!(created.long_paths_enabled, cfg!(windows));
        let folder = PathBuf::from(&wanted.path);
        assert_eq!(
            git_sync(&folder, &["branch", "--show-current"]),
            "ticket/ARC-0001-a"
        );
        assert_eq!(
            folders_exist(&[wanted.path.clone(), text(&dir.path().join("nope"))])
                .await
                .unwrap(),
            vec![true, false]
        );
        let again = create(&request(&root, "ticket/ARC-0001-b", None)).await;
        assert_eq!(again.unwrap_err().code(), "worktree-exists");
        let taken = create(&request(&root, "ticket/ARC-0001-a", None)).await;
        assert_eq!(taken.unwrap_err().code(), "branch-exists");
    }

    #[tokio::test]
    async fn refuses_a_repository_without_commits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("arches");
        fs::create_dir_all(&root).unwrap();
        git_sync(&root, &["init", "-q"]);
        let error = create(&request(&root, "ticket/ARC-0001-a", None))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "no-commit");
        assert!(!dir.path().join("arches-worktrees").exists());
    }

    #[tokio::test]
    async fn creates_a_missing_target_from_head_then_reuses_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("arches");
        repository(&root);
        let first = create(&request(&root, "ticket/ARC-0001-a", Some("release")))
            .await
            .unwrap();
        assert_eq!(first.target_created_from.as_deref(), Some("main"));
        let head = git_sync(&root, &["rev-parse", "refs/heads/release"]);
        assert_eq!(git_sync(&root, &["rev-parse", "HEAD"]), head);
        let upstream = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "release@{upstream}"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(!upstream.status.success());
        assert_eq!(ensure_target(&root, "release").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_detached_main_checkout_names_its_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("arches");
        repository(&root);
        git_sync(&root, &["checkout", "-q", "--detach"]);
        let short = git_sync(&root, &["rev-parse", "--short", "HEAD"]);
        assert_eq!(ensure_target(&root, "release").await.unwrap(), Some(short));
    }

    #[tokio::test]
    async fn creates_a_missing_target_from_the_origin_branch() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin");
        repository(&origin);
        git_sync(&origin, &["branch", "release"]);
        git_sync(&origin, &["checkout", "-q", "release"]);
        git_sync(
            &origin,
            &["commit", "-q", "--allow-empty", "-m", "on release"],
        );
        let wanted = git_sync(&origin, &["rev-parse", "HEAD"]);
        git_sync(&origin, &["checkout", "-q", "main"]);
        git_sync(dir.path(), &["clone", "-q", &text(&origin), "arches"]);
        let root = dir.path().join("arches");
        let created = create(&request(&root, "ticket/ARC-0001-a", Some("release")))
            .await
            .unwrap();
        assert_eq!(
            created.target_created_from.as_deref(),
            Some("origin/release")
        );
        assert_eq!(
            git_sync(&root, &["rev-parse", "refs/heads/release"]),
            wanted
        );
        let folder = dir.path().join("arches-worktrees").join("ARC-0001");
        assert_eq!(git_sync(&folder, &["rev-parse", "HEAD"]), wanted);
    }

    #[tokio::test]
    async fn long_paths_are_set_once_and_only_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("arches");
        repository(&root);
        assert!(!ensure_long_paths(&root, false).await.unwrap());
        assert!(ensure_long_paths(&root, true).await.unwrap());
        assert_eq!(
            git_sync(&root, &["config", "--get", "core.longpaths"]),
            "true"
        );
        assert!(!ensure_long_paths(&root, true).await.unwrap());
    }

    #[tokio::test]
    async fn failures_name_the_command_and_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let error = git(dir.path(), &["rev-parse", "--verify", "HEAD"])
            .await
            .unwrap_err();
        assert_eq!(error.code(), "git-failed");
        assert!(
            error
                .message()
                .starts_with("git failed (git rev-parse --verify HEAD): "),
            "{}",
            error.message()
        );
        let silent = git(dir.path(), &["config", "--get", "nullradix.missing"])
            .await
            .unwrap_err();
        assert_eq!(
            silent.message(),
            "git failed (git config --get nullradix.missing): \
             git config --get nullradix.missing exited with code 1"
        );
    }
}
