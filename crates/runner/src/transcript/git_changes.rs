//! Changes from git against the merge base when the session has its own worktree or branch.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use nullradix_app::AppError;
use nullradix_app::session_detail::{
    ChangedFile, FileDiff, FileStatus, change_not_found, check_change_path,
};
use nullradix_app::worktrees::git_failed;

use super::jsonl::count;
use super::unified_diff;
use crate::worktree::output;

/// Where and against what the session is compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitScope {
    pub(crate) root: PathBuf,
    /// The base branch, or `HEAD` when a worktree knows none.
    pub(crate) base: String,
    /// Merge base of `HEAD` and the base.
    pub(crate) merge_base: String,
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(AppError::internal)
}

/// Trimmed output when git succeeds, `None` otherwise.
async fn attempt(dir: &Path, args: &[&str]) -> Option<String> {
    let output = output(dir, args).await.ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Untrimmed output, for `-z` lists.
async fn raw(dir: &Path, args: &[&str]) -> Result<String, AppError> {
    let output = output(dir, args).await?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(git_failed(args, &detail))
}

/// The scope when `cwd` is a linked worktree or on another branch than the base; else none.
pub(crate) async fn scope(cwd: &Path) -> Result<Option<GitScope>, AppError> {
    let dir = cwd.to_path_buf();
    if !blocking(move || dir.is_dir()).await? {
        return Ok(None);
    }
    let Some(paths) = attempt(
        cwd,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ],
    )
    .await
    else {
        return Ok(None);
    };
    let lines: Vec<&str> = paths.lines().map(str::trim).collect();
    let [root, git_dir, common_dir] = lines.as_slice() else {
        return Ok(None);
    };
    let linked = normalize(git_dir) != normalize(common_dir);
    let root = PathBuf::from(root);
    let branch = attempt(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await;
    let base = base_branch(&root).await;
    let own_branch = match (&branch, &base) {
        (Some(branch), Some(base)) => {
            branch != base && Some(branch.as_str()) != base.strip_prefix("origin/")
        }
        _ => false,
    };
    if !linked && !own_branch {
        return Ok(None);
    }
    let base = base.unwrap_or_else(|| String::from("HEAD"));
    let Some(merge_base) = attempt(&root, &["merge-base", "HEAD", &base])
        .await
        .filter(|found| !found.is_empty())
    else {
        return Ok(None);
    };
    Ok(Some(GitScope {
        root,
        base,
        merge_base,
    }))
}

/// Changed files, committed, uncommitted and new, by path.
pub(crate) async fn files(scope: &GitScope) -> Result<Vec<ChangedFile>, AppError> {
    let root = &scope.root;
    let base = scope.merge_base.as_str();
    let stats = numstat(&raw(root, &["diff", "-M", "--numstat", "-z", base]).await?);
    let entries = name_status(&raw(root, &["diff", "-M", "--name-status", "-z", base]).await?);
    let mut files: Vec<ChangedFile> = entries
        .into_iter()
        .map(|entry| {
            let stat = stats.get(&entry.path);
            ChangedFile {
                status: status_of(entry.status),
                added: stat.map_or(0, |stat| stat.0),
                removed: stat.map_or(0, |stat| stat.1),
                path: entry.path,
            }
        })
        .collect();
    for path in untracked(root).await? {
        let content = read_inside(root, &path).await?;
        files.push(ChangedFile {
            added: content.map_or(0, |text| count(text.lines().count())),
            path,
            status: FileStatus::Added,
            removed: 0,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Diff of a listed file; other paths are refused or not found.
pub(crate) async fn file(scope: &GitScope, path: &str) -> Result<FileDiff, AppError> {
    check_change_path(path)?;
    let root = &scope.root;
    if untracked(root).await?.iter().any(|file| file == path) {
        let content = read_inside(root, path).await?.unwrap_or_default();
        return Ok(unified_diff::all_added(path, &content, None));
    }
    let base = scope.merge_base.as_str();
    let entry = name_status(&raw(root, &["diff", "-M", "--name-status", "-z", base]).await?)
        .into_iter()
        .find(|entry| entry.path == path)
        .ok_or_else(|| change_not_found(path))?;
    let mut args = vec!["diff", "-M", base, "--"];
    if let Some(old) = &entry.old_path {
        args.push(old);
    }
    args.push(&entry.path);
    let output = raw(root, &args).await?;
    Ok(unified_diff::parse(path, &output))
}

/// `main`, else the remote's default branch.
async fn base_branch(root: &Path) -> Option<String> {
    if attempt(
        root,
        &["rev-parse", "--verify", "--quiet", "refs/heads/main"],
    )
    .await
    .is_some()
    {
        return Some(String::from("main"));
    }
    attempt(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .await
    .filter(|remote| !remote.is_empty())
}

/// New files that are neither tracked nor ignored.
async fn untracked(root: &Path) -> Result<Vec<String>, AppError> {
    Ok(
        raw(root, &["ls-files", "--others", "--exclude-standard", "-z"])
            .await?
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

/// Text of a file only when it resolves inside `root`; links leading out read nothing.
async fn read_inside(root: &Path, path: &str) -> Result<Option<String>, AppError> {
    let root = root.to_path_buf();
    let path = path.to_owned();
    blocking(move || {
        let real_root = fs::canonicalize(&root).ok()?;
        let real = fs::canonicalize(root.join(&path)).ok()?;
        if !real.starts_with(&real_root) {
            return None;
        }
        String::from_utf8(fs::read(real).ok()?).ok()
    })
    .await
}

fn status_of(letter: char) -> FileStatus {
    match letter {
        'A' | 'C' => FileStatus::Added,
        'D' => FileStatus::Deleted,
        'R' => FileStatus::Renamed,
        _ => FileStatus::Modified,
    }
}

/// Comparable spelling of a path.
fn normalize(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = path.trim_end_matches('/');
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.to_owned()
    }
}

struct StatusEntry {
    path: String,
    old_path: Option<String>,
    status: char,
}

/// Fields of a `-z` output without the empty last one.
fn fields(out: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = out.split('\0').collect();
    if parts.last().is_some_and(|last| last.is_empty()) {
        parts.pop();
    }
    parts
}

/// `--name-status -z`: status and path; renames and copies carry the old path first.
fn name_status(out: &str) -> Vec<StatusEntry> {
    let parts = fields(out);
    let field = |index: usize| {
        parts
            .get(index)
            .map_or_else(String::new, |part| (*part).to_owned())
    };
    let mut list = Vec::new();
    let mut index = 0;
    while index < parts.len() {
        let Some(status) = parts[index].chars().next() else {
            index += 1;
            continue;
        };
        if status == 'R' || status == 'C' {
            list.push(StatusEntry {
                status,
                old_path: Some(field(index + 1)),
                path: field(index + 2),
            });
            index += 3;
        } else {
            list.push(StatusEntry {
                status,
                old_path: None,
                path: field(index + 1),
            });
            index += 2;
        }
    }
    list
}

/// `--numstat -z`: added, removed and path; a rename leaves the path empty and two follow.
fn numstat(out: &str) -> HashMap<String, (i64, i64)> {
    let parts = fields(out);
    let is_count = |field: &str| field == "-" || field.bytes().all(|byte| byte.is_ascii_digit());
    let mut map = HashMap::new();
    let mut index = 0;
    while index < parts.len() {
        let record: Vec<&str> = parts[index].splitn(3, '\t').collect();
        if record.len() < 3 || !is_count(record[0]) || !is_count(record[1]) {
            index += 1;
            continue;
        }
        // binary files show `-` and count nothing
        let added = record[0].parse().unwrap_or(0);
        let removed = record[1].parse().unwrap_or(0);
        let path = if record[2].is_empty() {
            let path = parts
                .get(index + 2)
                .map_or_else(String::new, |part| (*part).to_owned());
            index += 2;
            path
        } else {
            record[2].to_owned()
        };
        map.insert(path, (added, removed));
        index += 1;
    }
    map
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use nullradix_app::session_detail::DiffLineKind;

    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }

    /// A repository on `main` with two files.
    fn repository(dir: &Path) -> PathBuf {
        let root = dir.join("repo");
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        fs::write(root.join("old.txt"), "a\nb\nc\n").unwrap();
        fs::write(root.join("gone.txt"), "gone\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "base"]);
        root
    }

    fn worktree(root: &Path, name: &str) -> PathBuf {
        let path = root.parent().unwrap().join(name);
        git(
            root,
            &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()],
        );
        path
    }

    #[tokio::test]
    async fn the_main_checkout_on_main_has_no_scope() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        fs::write(root.join("old.txt"), "changed\n").unwrap();
        assert_eq!(scope(&root).await.unwrap(), None);
        assert_eq!(scope(dir.path()).await.unwrap(), None);
        assert_eq!(scope(&dir.path().join("missing")).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_worktree_compares_with_the_merge_base_with_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        let tree = worktree(&root, "wt-1");
        fs::write(tree.join("old.txt"), "a\nB\nc\n").unwrap();
        git(&tree, &["commit", "-q", "-am", "ticket"]);
        fs::remove_file(tree.join("gone.txt")).unwrap();
        fs::write(tree.join("new.txt"), "one\ntwo\n").unwrap();
        fs::write(root.join("main-only.txt"), "x").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "main only"]);
        let found = scope(&tree).await.unwrap().expect("the worktree is found");
        assert_eq!(found.base, "main");
        assert_eq!(
            files(&found).await.unwrap(),
            vec![
                ChangedFile {
                    path: "gone.txt".into(),
                    status: FileStatus::Deleted,
                    added: 0,
                    removed: 1
                },
                ChangedFile {
                    path: "new.txt".into(),
                    status: FileStatus::Added,
                    added: 2,
                    removed: 0
                },
                ChangedFile {
                    path: "old.txt".into(),
                    status: FileStatus::Modified,
                    added: 1,
                    removed: 1
                },
            ]
        );
        let diff = file(&found, "old.txt").await.unwrap();
        let kinds: Vec<DiffLineKind> = diff.hunks[0].lines.iter().map(|line| line.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiffLineKind::Context,
                DiffLineKind::Remove,
                DiffLineKind::Add,
                DiffLineKind::Context
            ]
        );
        assert_eq!(
            file(&found, "new.txt").await.unwrap().hunks[0].header,
            "@@ -0,0 +1,2 @@"
        );
    }

    #[tokio::test]
    async fn an_own_branch_in_the_main_checkout_counts_too() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        git(&root, &["checkout", "-q", "-b", "ticket/x"]);
        fs::write(root.join("t.txt"), "t").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "t"]);
        let found = scope(&root).await.unwrap().expect("the branch is found");
        let listed = files(&found).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "t.txt");
        assert_eq!(listed[0].status, FileStatus::Added);
    }

    #[tokio::test]
    async fn refuses_paths_outside_and_files_not_changed() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        let tree = worktree(&root, "wt-2");
        let found = scope(&tree).await.unwrap().unwrap();
        for path in ["../x", "/etc/passwd", "C:/x", "-p", "a/../../b", ""] {
            let error = file(&found, path).await.unwrap_err();
            assert_eq!(error.code(), "change-path-invalid", "{path}");
        }
        let error = file(&found, "old.txt").await.unwrap_err();
        assert_eq!(error.code(), "change-not-found");
    }

    #[test]
    fn parses_name_status_and_numstat_with_renames() {
        let entries = name_status("M\0a.txt\0R100\0old.txt\0new.txt\0");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].status, 'R');
        assert_eq!(entries[1].old_path.as_deref(), Some("old.txt"));
        assert_eq!(entries[1].path, "new.txt");
        let stats = numstat("1\t2\ta.txt\0-\t-\tb.bin\0");
        assert_eq!(stats["a.txt"], (1, 2));
        assert_eq!(stats["b.bin"], (0, 0));
        let renamed = numstat("3\t0\t\0old.txt\0new.txt\0");
        assert_eq!(renamed["new.txt"], (3, 0));
    }
}
