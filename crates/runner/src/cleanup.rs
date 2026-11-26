//! Removing a ready worktree as the legacy cleaner did; the branch always stays.

use std::fs;
use std::path::{Path, PathBuf};

use nullradix_app::AppError;
use nullradix_app::worktrees::{RemoveWorktree, WorktreeRemoval};

use crate::git::local_key;
use crate::worktree::git;

/// Files the harness writes into a worktree itself; they do not count as changes.
const HARNESS_FILES: [&str; 7] = [
    ".claude/settings.local.json",
    ".claude/rules/nullradix-docs.md",
    ".claude/rules/nullradix-board.md",
    ".claude/rules/ai-harness-docs.md",
    ".claude/rules/ai-harness-board.md",
    ".claude/rules/nexus-docs.md",
    ".claude/rules/nexus-board.md",
];

pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(AppError::internal)
}

fn kept(reason: impl Into<String>) -> WorktreeRemoval {
    WorktreeRemoval::Kept(reason.into())
}

/// Removes the worktree when it exists, does not hold the caller and is clean.
pub(crate) async fn remove(request: &RemoveWorktree) -> Result<WorktreeRemoval, AppError> {
    let path = PathBuf::from(&request.path);
    let (folder, cwd) = (path.clone(), PathBuf::from(&request.cwd));
    let (exists, inside) = blocking(move || (folder.exists(), contains(&folder, &cwd))).await?;
    if !exists {
        return Ok(WorktreeRemoval::Missing);
    }
    if inside {
        return Ok(kept("the current folder is inside it"));
    }
    if let Some(reason) = dirty(&path).await {
        return Ok(kept(reason));
    }
    let failed = blocking(move || remove_links(&path)).await?;
    if let Some(link) = failed.first() {
        return Ok(kept(format!(
            "the link {} inside could not be removed",
            link.display()
        )));
    }
    let root = Path::new(&request.root);
    // --force only for the harness files git does not know; no links are left
    match git(root, &["worktree", "remove", "--force", &request.path]).await {
        Ok(_) => Ok(WorktreeRemoval::Removed),
        // git drops its entry even when locked files leave a rest of the folder
        Err(error) => match listed(root, &request.path).await {
            Some(false) => Ok(WorktreeRemoval::Removed),
            _ => Ok(kept(error.message())),
        },
    }
}

/// Whether `path` is the folder `outer` or lies inside it.
fn contains(outer: &Path, path: &Path) -> bool {
    let outer = local_key(outer);
    let inner = local_key(path);
    inner == outer || inner.starts_with(&format!("{outer}/"))
}

/// Why the worktree counts as changed, or `None` when only harness files differ.
async fn dirty(path: &Path) -> Option<String> {
    let status = match git(path, &["status", "--porcelain", "--untracked-files=all"]).await {
        Ok(status) => status,
        Err(error) => return Some(error.message().to_owned()),
    };
    status
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .map(|line| strip_status(line.trim()).replace('\\', "/"))
        .find(|file| !HARNESS_FILES.contains(&file.as_str()))
        .map(|file| format!("it has uncommitted changes, such as {file}"))
}

/// The line without its status letters, which the trimmed output leaves unaligned.
fn strip_status(line: &str) -> &str {
    match line.split_once(char::is_whitespace) {
        Some((_, rest)) => rest.trim_start(),
        None => line,
    }
}

/// Whether git lists the folder as a worktree; `None` when git cannot tell.
pub(crate) async fn listed(root: &Path, path: &str) -> Option<bool> {
    let list = git(root, &["worktree", "list", "--porcelain"]).await.ok()?;
    let path = PathBuf::from(path);
    blocking(move || {
        let wanted = local_key(&path);
        list.split('\n')
            .filter_map(|line| line.trim().strip_prefix("worktree "))
            .any(|found| local_key(Path::new(found)) == wanted)
    })
    .await
    .ok()
}

/// Removes every symlink and junction below `root`, never their targets; returns the failures.
pub(crate) fn remove_links(root: &Path) -> Vec<PathBuf> {
    if is_link(root) {
        return vec![root.to_path_buf()];
    }
    let mut found = Vec::new();
    let mut failed = Vec::new();
    visit(root, &mut found, &mut failed);
    failed.extend(found.into_iter().filter(|link| !remove_link(link)));
    failed
}

/// Collects links without following them; unreadable folders may hide one and count as failed.
fn visit(dir: &Path, found: &mut Vec<PathBuf>, unreadable: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        unreadable.push(dir.to_path_buf());
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            unreadable.push(dir.to_path_buf());
            continue;
        };
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            unreadable.push(path);
            continue;
        };
        if meta.file_type().is_symlink() {
            found.push(path);
        } else if meta.is_dir() {
            visit(&path, found, unreadable);
        }
    }
}

/// Removes exactly the link: a file link as a file, a junction or folder link as an empty folder.
fn remove_link(path: &Path) -> bool {
    if fs::remove_file(path).is_err() && fs::remove_dir(path).is_err() {
        return false;
    }
    fs::symlink_metadata(path).is_err()
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
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

    /// A repository ignoring `node_modules` with the worktree `ARC-0001` on `ticket/a`.
    fn setup(dir: &Path) -> (PathBuf, PathBuf) {
        let root = dir.join("arches");
        fs::create_dir_all(&root).unwrap();
        git_sync(&root, &["init", "-q", "-b", "main"]);
        fs::write(root.join(".gitignore"), "node_modules\n").unwrap();
        git_sync(&root, &["add", ".gitignore"]);
        git_sync(&root, &["commit", "-q", "-m", "init"]);
        let path = dir.join("arches-worktrees").join("ARC-0001");
        git_sync(
            &root,
            &["worktree", "add", "-q", "-b", "ticket/a", &text(&path)],
        );
        (root, path)
    }

    fn request(root: &Path, path: &Path, cwd: &Path) -> RemoveWorktree {
        RemoveWorktree {
            root: text(root),
            path: text(path),
            cwd: text(cwd),
        }
    }

    fn branch_kept(root: &Path) -> bool {
        git_sync(root, &["branch", "--list", "ticket/a"]).contains("ticket/a")
    }

    fn worktree_listed(root: &Path) -> bool {
        git_sync(root, &["worktree", "list", "--porcelain"]).contains("ARC-0001")
    }

    #[tokio::test]
    async fn removes_a_clean_worktree_and_keeps_its_branch() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        let harness = path.join(".claude");
        fs::create_dir_all(harness.join("rules")).unwrap();
        fs::write(harness.join("settings.local.json"), "{}").unwrap();
        fs::write(harness.join("rules").join("nexus-board.md"), "x").unwrap();
        let removal = remove(&request(&root, &path, &root)).await.unwrap();
        assert_eq!(removal, WorktreeRemoval::Removed);
        assert!(!path.exists());
        assert!(!worktree_listed(&root));
        assert!(branch_kept(&root));
    }

    #[tokio::test]
    async fn keeps_a_worktree_with_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        fs::write(path.join("new.txt"), "x").unwrap();
        let removal = remove(&request(&root, &path, &root)).await.unwrap();
        assert_eq!(removal, kept("it has uncommitted changes, such as new.txt"));
        assert!(path.join("new.txt").exists());
        assert!(worktree_listed(&root));
    }

    #[tokio::test]
    async fn keeps_the_worktree_the_caller_is_in() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        let inner = path.join("src");
        fs::create_dir_all(&inner).unwrap();
        for cwd in [&path, &inner] {
            let removal = remove(&request(&root, &path, cwd)).await.unwrap();
            assert_eq!(removal, kept("the current folder is inside it"));
        }
        assert!(path.exists());
        let sibling = dir.path().join("arches-worktrees").join("ARC-0001-b");
        assert!(!contains(&path, &sibling));
    }

    #[tokio::test]
    async fn skips_a_missing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        let gone = dir.path().join("arches-worktrees").join("ARC-0002");
        let removal = remove(&request(&root, &gone, &root)).await.unwrap();
        assert_eq!(removal, WorktreeRemoval::Missing);
        assert!(path.exists());
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
    async fn removes_links_inside_but_never_their_targets() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        let target = root.join("node_modules");
        fs::create_dir_all(target.join(".bin")).unwrap();
        fs::write(target.join(".bin").join("tool"), "x").unwrap();
        let link = path.join("node_modules");
        link_dir(&target, &link);
        assert!(is_link(&link));
        let removal = remove(&request(&root, &path, &root)).await.unwrap();
        assert_eq!(removal, WorktreeRemoval::Removed);
        assert!(!path.exists());
        assert_eq!(
            fs::read_to_string(target.join(".bin").join("tool")).unwrap(),
            "x"
        );
        assert!(branch_kept(&root));
    }

    #[test]
    fn a_folder_that_is_a_link_itself_is_never_touched() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep.txt"), "x").unwrap();
        let link = dir.path().join("linked");
        link_dir(&target, &link);
        assert_eq!(remove_links(&link), vec![link.clone()]);
        assert!(target.join("keep.txt").exists());
    }

    #[tokio::test]
    async fn a_locked_worktree_stays_with_the_git_error() {
        let dir = tempfile::tempdir().unwrap();
        let (root, path) = setup(dir.path());
        git_sync(&root, &["worktree", "lock", &text(&path)]);
        let removal = remove(&request(&root, &path, &root)).await.unwrap();
        let WorktreeRemoval::Kept(reason) = removal else {
            panic!("a locked worktree was removed");
        };
        assert!(
            reason.starts_with("git failed (git worktree remove --force "),
            "{reason}"
        );
        assert!(path.exists() && worktree_listed(&root));
    }

    #[tokio::test]
    async fn a_failure_on_a_folder_git_does_not_list_counts_as_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _) = setup(dir.path());
        let foreign = dir.path().join("foreign");
        fs::create_dir_all(&foreign).unwrap();
        git_sync(&foreign, &["init", "-q", "-b", "main"]);
        git_sync(&foreign, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let removal = remove(&request(&root, &foreign, &root)).await.unwrap();
        assert_eq!(removal, WorktreeRemoval::Removed);
    }

    #[test]
    fn status_letters_are_cut_off_trimmed_lines() {
        assert_eq!(strip_status("?? a b.txt"), "a b.txt");
        assert_eq!(strip_status("M src/x.rs"), "src/x.rs");
        assert_eq!(strip_status("single"), "single");
    }
}
