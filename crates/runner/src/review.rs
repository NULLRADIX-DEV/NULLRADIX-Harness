//! Reads what a ticket's branch changed and the files at its head, as legacy `ReviewDiff`.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use nullradix_app::AppError;
use nullradix_app::review::{
    BranchChanges, BranchFile, ChangedFile, FilePatch, MAX_FILE_BYTES, MAX_PATCH_BYTES,
    MAX_REVIEW_FILES, ReviewRequest, check_branch, check_review_path, review_file_unknown,
};
use nullradix_app::worktrees::git_failed;
use tokio::io::AsyncReadExt;

use crate::worktree::{attempt, git, has_ref};

/// Bytes of the file lists git writes; a branch with more shows the files read until then.
const MAX_LIST_BYTES: usize = 16 * 1024 * 1024;
/// Bytes of error output kept from a git call.
const MAX_ERROR_BYTES: u64 = 64 * 1024;
/// Bytes looked at for a NUL, as git does to tell binary files.
const BINARY_SCAN: usize = 8000;

/// What a capped read of git's output left.
struct Capped {
    bytes: Vec<u8>,
    /// The output was longer than the cap and the call was stopped.
    cut: bool,
}

/// Runs git with argv and no shell and keeps at most `cap` bytes of its output.
async fn capped(cwd: &Path, args: &[&str], cap: usize) -> Result<Capped, AppError> {
    let mut command = tokio::process::Command::new("git");
    command
        .args([
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.quotepath=false",
        ])
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|error| git_failed(args, &error.to_string()))?;
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(git_failed(args, "no output pipe"));
    };
    let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
    let mut bytes = Vec::new();
    let read = async {
        let mut reader = (&mut stdout).take(limit);
        let read = reader.read_to_end(&mut bytes).await;
        // git may still be writing and would block on the full pipe, so it is stopped here
        if bytes.len() > cap {
            let _ = child.start_kill();
        }
        read
    };
    let mut detail = Vec::new();
    let errors = async {
        let mut reader = (&mut stderr).take(MAX_ERROR_BYTES);
        reader.read_to_end(&mut detail).await?;
        // the rest is read and dropped, so git never blocks on a full pipe
        tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await
    };
    let (read, errors) = tokio::join!(read, errors);
    read.map_err(|error| git_failed(args, &error.to_string()))?;
    errors.map_err(|error| git_failed(args, &error.to_string()))?;
    let cut = bytes.len() > cap;
    bytes.truncate(cap);
    let status = child
        .wait()
        .await
        .map_err(|error| git_failed(args, &error.to_string()))?;
    if !cut && !status.success() {
        let text = String::from_utf8_lossy(&detail).trim().to_owned();
        let reason = if text.is_empty() {
            format!("git {} did not succeed", args.join(" "))
        } else {
            text
        };
        return Err(git_failed(args, &reason));
    }
    Ok(Capped { bytes, cut })
}

/// A reference git resolves for the branch: the local branch when there is one.
async fn branch_ref(root: &Path, branch: &str) -> Result<String, AppError> {
    check_branch(branch)?;
    let local = format!("refs/heads/{branch}");
    Ok(if has_ref(root, &local).await? {
        local
    } else {
        branch.to_owned()
    })
}

/// What the branch is compared with: the local target branch, else the head of the main checkout.
struct Base {
    reference: String,
    name: Option<String>,
}

async fn base_of(root: &Path, target: Option<&str>) -> Result<Base, AppError> {
    let head = Base {
        reference: String::from("HEAD"),
        name: None,
    };
    let Some(target) = target.filter(|name| !name.is_empty()) else {
        return Ok(head);
    };
    check_branch(target)?;
    let reference = format!("refs/heads/{target}");
    if has_ref(root, &reference).await? {
        Ok(Base {
            reference,
            name: Some(target.to_owned()),
        })
    } else {
        Ok(head)
    }
}

/// What the branch is compared by: the range, the base and where they meet.
struct Compared {
    range: String,
    base: Base,
    merge_base: String,
}

async fn compare(request: &ReviewRequest) -> Result<Compared, AppError> {
    let root = Path::new(&request.root);
    let branch = branch_ref(root, &request.branch).await?;
    let base = base_of(root, request.target.as_deref()).await?;
    let merge_base = git(root, &["merge-base", &base.reference, &branch]).await?;
    let range = format!("{}...{branch}", base.reference);
    Ok(Compared {
        range,
        base,
        merge_base,
    })
}

const DIFF: [&str; 5] = ["diff", "-M", "--no-ext-diff", "--no-textconv", "-z"];

async fn list(request: &ReviewRequest, max_files: usize) -> Result<BranchChanges, AppError> {
    let root = Path::new(&request.root);
    let Compared {
        range,
        base,
        merge_base,
    } = compare(request).await?;
    let counted = capped(
        root,
        &[&DIFF[..], &["--numstat", range.as_str(), "--"]].concat(),
        MAX_LIST_BYTES,
    )
    .await?;
    let named = capped(
        root,
        &[&DIFF[..], &["--name-status", range.as_str(), "--"]].concat(),
        MAX_LIST_BYTES,
    )
    .await?;
    let cut = named.cut;
    // the lists are large, so parsing them stays off the async runtime
    let (lines, mut entries) = tokio::task::spawn_blocking(move || {
        (
            numstat_of(&String::from_utf8_lossy(&counted.bytes)),
            name_status_of(&String::from_utf8_lossy(&named.bytes)),
        )
    })
    .await
    .map_err(AppError::internal)?;
    let truncated = cut || entries.len() > max_files;
    entries.truncate(max_files);
    let files = entries
        .into_iter()
        .map(|entry| {
            let stat = lines.get(&entry.path);
            ChangedFile {
                path: entry.path,
                old_path: entry.old_path,
                status: entry.status,
                additions: stat.map_or(0, |stat| stat.additions),
                deletions: stat.map_or(0, |stat| stat.deletions),
                binary: stat.is_some_and(|stat| stat.binary),
            }
        })
        .collect();
    let target = match base.name {
        Some(name) => name,
        None => attempt(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .await?
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| String::from("HEAD")),
    };
    Ok(BranchChanges {
        target,
        base: merge_base,
        files,
        truncated,
    })
}

/// The files the branch changed against its target.
pub(crate) async fn changes(request: &ReviewRequest) -> Result<BranchChanges, AppError> {
    list(request, MAX_REVIEW_FILES).await
}

/// The status of one path of the changes, with the earlier path of a rename; none if unchanged.
async fn entry_of(root: &Path, range: &str, path: &str) -> Result<Option<StatusEntry>, AppError> {
    let one = capped(
        root,
        &[&DIFF[..], &["--name-status", range, "--", path]].concat(),
        MAX_LIST_BYTES,
    )
    .await?;
    let found = name_status_of(&String::from_utf8_lossy(&one.bytes))
        .into_iter()
        .find(|entry| entry.path == path);
    // limited to its own path a rename shows as an added file, so only then are all names read
    if found.as_ref().is_some_and(|entry| entry.status == "A") {
        let all = capped(
            root,
            &[&DIFF[..], &["--name-status", range, "--"]].concat(),
            MAX_LIST_BYTES,
        )
        .await?;
        let renamed = name_status_of(&String::from_utf8_lossy(&all.bytes))
            .into_iter()
            .find(|entry| entry.path == path);
        return Ok(renamed.or(found));
    }
    Ok(found)
}

/// The patch of one file of the changes; a path outside the changes is refused.
pub(crate) async fn patch(request: &ReviewRequest, path: &str) -> Result<FilePatch, AppError> {
    check_review_path(path, &request.branch)?;
    let root = Path::new(&request.root);
    let compared = compare(request).await?;
    let range = compared.range.as_str();
    let entry = entry_of(root, range, path)
        .await?
        .ok_or_else(|| review_file_unknown(path, &request.branch))?;
    let mut args = vec![
        "diff",
        "-M",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        range,
        "--",
    ];
    if let Some(old) = &entry.old_path {
        args.push(old);
    }
    args.push(&entry.path);
    let read = capped(root, &args, MAX_PATCH_BYTES).await?;
    Ok(FilePatch {
        patch: utf8_prefix(&read.bytes),
        truncated: read.cut,
    })
}

/// A file as it is at the head of the branch; directories and submodules are no files.
pub(crate) async fn file(root: &str, branch: &str, path: &str) -> Result<BranchFile, AppError> {
    check_review_path(path, branch)?;
    let root = Path::new(root);
    let reference = branch_ref(root, branch).await?;
    let spec = format!("{reference}^{{commit}}:{path}");
    let Some(object) = attempt(root, &["rev-parse", "--verify", "--quiet", &spec]).await? else {
        return Err(review_file_unknown(path, branch));
    };
    let kind = git(root, &["cat-file", "-t", &object]).await?;
    if kind != "blob" {
        return Err(review_file_unknown(path, branch));
    }
    let size: u64 = git(root, &["cat-file", "-s", &object])
        .await?
        .parse()
        .map_err(|_| git_failed(&["cat-file", "-s"], "the size is no number"))?;
    let read = capped(root, &["cat-file", "blob", &object], MAX_FILE_BYTES).await?;
    let binary = read.bytes.iter().take(BINARY_SCAN).any(|byte| *byte == 0);
    Ok(BranchFile {
        size,
        binary,
        text: (!binary).then(|| utf8_prefix(&read.bytes)),
        truncated: read.cut,
    })
}

/// The bytes as text; a character cut off at the end is dropped, others turn to U+FFFD.
fn utf8_prefix(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(error) if error.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..error.valid_up_to()]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

struct StatusEntry {
    path: String,
    old_path: Option<String>,
    status: String,
}

struct Stat {
    additions: u64,
    deletions: u64,
    binary: bool,
}

/// The fields of a `-z` output without the last one, which is empty or cut off by the cap.
fn fields(output: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = output.split('\0').collect();
    // the last part is empty after a complete output and half written after a cut one
    parts.pop();
    parts
}

/// `--name-status -z`: status and path; renames and copies give status, old path and new path.
fn name_status_of(output: &str) -> Vec<StatusEntry> {
    let parts = fields(output);
    let mut list = Vec::new();
    let mut index = 0;
    while index < parts.len() {
        let Some(status) = parts[index].chars().next() else {
            index += 1;
            continue;
        };
        let at = |offset: usize| {
            parts
                .get(index + offset)
                .copied()
                .unwrap_or_default()
                .to_owned()
        };
        if status == 'R' || status == 'C' {
            list.push(StatusEntry {
                status: status.to_string(),
                old_path: Some(at(1)),
                path: at(2),
            });
            index += 3;
        } else {
            list.push(StatusEntry {
                status: status.to_string(),
                old_path: None,
                path: at(1),
            });
            index += 2;
        }
    }
    // a record cut off by the cap has no path
    list.retain(|entry| !entry.path.is_empty());
    list
}

/// `--numstat -z`: `added TAB removed TAB path`; a rename has an empty path and then both paths.
fn numstat_of(output: &str) -> HashMap<String, Stat> {
    let parts = fields(output);
    let is_count = |field: &str| {
        field == "-" || (!field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit()))
    };
    let mut map = HashMap::new();
    let mut index = 0;
    while index < parts.len() {
        let record: Vec<&str> = parts[index].splitn(3, '\t').collect();
        if record.len() < 3 || !is_count(record[0]) || !is_count(record[1]) {
            index += 1;
            continue;
        }
        let binary = record[0] == "-" && record[1] == "-";
        let count = |field: &str| {
            if binary {
                0
            } else {
                field.parse().unwrap_or(0)
            }
        };
        let stat = Stat {
            additions: count(record[0]),
            deletions: count(record[1]),
            binary,
        };
        let path = if record[2].is_empty() {
            let path = parts.get(index + 2).copied().unwrap_or_default().to_owned();
            index += 2;
            path
        } else {
            record[2].to_owned()
        };
        map.insert(path, stat);
        index += 1;
    }
    map
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::*;

    const CAP: usize = 3;

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn commit(dir: &Path, files: &[(&str, &[u8])], message: &str) {
        for (name, content) in files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, content).unwrap();
        }
        git_in(dir, &["add", "-A"]);
        git_in(dir, &["commit", "-q", "-m", message]);
    }

    /// A base commit, a ticket branch (change, new, binary, deleted, renamed) and a commit on main.
    fn repository() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        git_in(&root, &["init", "-q", "-b", "main"]);
        commit(
            &root,
            &[
                ("old.txt", b"a\nb\nc\n"),
                ("gone.txt", b"gone\n"),
                ("move.txt", b"one\ntwo\nthree\nfour\nfive\n"),
            ],
            "base",
        );
        git_in(&root, &["checkout", "-q", "-b", "ticket/x"]);
        commit(
            &root,
            &[
                ("old.txt", b"a\nnew\nc\n"),
                ("new.txt", b"new\n"),
                ("image.bin", &[0, 1, 2, 0, 255]),
                ("src/deep/file.rs", b"fn main() {}\n"),
            ],
            "ticket",
        );
        git_in(&root, &["rm", "-q", "gone.txt"]);
        git_in(&root, &["mv", "move.txt", "moved.txt"]);
        git_in(&root, &["commit", "-q", "-m", "ticket 2"]);
        git_in(&root, &["checkout", "-q", "main"]);
        commit(&root, &[("only-main.txt", b"main\n")], "main");
        (dir, root)
    }

    fn request(root: &Path, target: Option<&str>) -> ReviewRequest {
        ReviewRequest {
            root: root.to_string_lossy().into_owned(),
            branch: String::from("ticket/x"),
            target: target.map(str::to_owned),
        }
    }

    fn by_path<'a>(changes: &'a BranchChanges, path: &str) -> &'a ChangedFile {
        changes.files.iter().find(|file| file.path == path).unwrap()
    }

    #[tokio::test]
    async fn lists_the_changes_from_the_merge_base() {
        let (_dir, root) = repository();
        let changes = changes(&request(&root, None)).await.unwrap();
        assert_eq!(changes.target, "main");
        assert_eq!(
            changes.base,
            git_in(&root, &["merge-base", "main", "ticket/x"])
        );
        let mut paths: Vec<&str> = changes
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        paths.sort_unstable();
        assert_eq!(
            paths,
            [
                "gone.txt",
                "image.bin",
                "moved.txt",
                "new.txt",
                "old.txt",
                "src/deep/file.rs"
            ]
        );
        let old = by_path(&changes, "old.txt");
        assert_eq!(
            (old.status.as_str(), old.additions, old.deletions),
            ("M", 1, 1)
        );
        let new = by_path(&changes, "new.txt");
        assert_eq!(
            (new.status.as_str(), new.additions, new.deletions),
            ("A", 1, 0)
        );
        let gone = by_path(&changes, "gone.txt");
        assert_eq!(
            (gone.status.as_str(), gone.additions, gone.deletions),
            ("D", 0, 1)
        );
        assert!(!changes.truncated);
    }

    #[tokio::test]
    async fn detects_renames_and_binary_files() {
        let (_dir, root) = repository();
        let changes = changes(&request(&root, None)).await.unwrap();
        let moved = by_path(&changes, "moved.txt");
        assert_eq!(moved.status, "R");
        assert_eq!(moved.old_path.as_deref(), Some("move.txt"));
        assert_eq!((moved.additions, moved.deletions), (0, 0));
        let image = by_path(&changes, "image.bin");
        assert!(image.binary);
        assert_eq!((image.additions, image.deletions), (0, 0));
        assert!(!by_path(&changes, "old.txt").binary);
    }

    #[tokio::test]
    async fn compares_with_the_target_branch_when_it_exists_else_with_head() {
        let (_dir, root) = repository();
        git_in(&root, &["branch", "release/1", "main"]);
        let released = changes(&request(&root, Some("release/1"))).await.unwrap();
        assert_eq!(released.target, "release/1");
        assert_eq!(
            released.base,
            git_in(&root, &["merge-base", "release/1", "ticket/x"])
        );
        let missing = changes(&request(&root, Some("release/2"))).await.unwrap();
        assert_eq!(missing.target, "main");
        assert_eq!(missing.files.len(), 6);
    }

    #[tokio::test]
    async fn cuts_a_patch_to_the_limit_and_says_so() {
        let (_dir, root) = repository();
        git_in(&root, &["checkout", "-q", "ticket/x"]);
        let big = "a line with some text\n".repeat(MAX_PATCH_BYTES / 20);
        commit(&root, &[("big.txt", big.as_bytes())], "big");
        git_in(&root, &["checkout", "-q", "main"]);
        let patch = patch(&request(&root, None), "big.txt").await.unwrap();
        assert!(patch.truncated);
        assert!(patch.patch.len() <= MAX_PATCH_BYTES);
        let small = self::patch(&request(&root, None), "old.txt").await.unwrap();
        assert!(!small.truncated);
        assert!(small.patch.contains("+new"));
        assert!(small.patch.contains("-b"));
    }

    #[tokio::test]
    async fn lists_no_more_files_than_the_limit_and_says_so() {
        let (_dir, root) = repository();
        git_in(&root, &["checkout", "-q", "ticket/x"]);
        let names: Vec<String> = (0..=CAP + 2)
            .map(|number| format!("many/f{number}.txt"))
            .collect();
        let files: Vec<(&str, &[u8])> = names
            .iter()
            .map(|name| (name.as_str(), &b"x\n"[..]))
            .collect();
        commit(&root, &files, "many");
        git_in(&root, &["checkout", "-q", "main"]);
        let listed = list(&request(&root, None), CAP).await.unwrap();
        assert!(listed.truncated);
        assert_eq!(listed.files.len(), CAP);
        let all = list(&request(&root, None), MAX_REVIEW_FILES).await.unwrap();
        assert!(!all.truncated);
    }

    #[tokio::test]
    async fn patches_of_renamed_and_binary_files() {
        let (_dir, root) = repository();
        let moved = patch(&request(&root, None), "moved.txt").await.unwrap();
        assert!(moved.patch.contains("rename to moved.txt"));
        let image = patch(&request(&root, None), "image.bin").await.unwrap();
        assert!(image.patch.contains("Binary files"));
    }

    fn blob(root: &Path, text: &str) -> String {
        let mut child = Command::new("git")
            .args(["hash-object", "-w", "--stdin"])
            .current_dir(root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), text.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[tokio::test]
    async fn a_path_with_glob_characters_names_only_that_file() {
        let (_dir, root) = repository();
        let star = blob(&root, "star\n");
        let plain = blob(&root, "plain\n");
        git_in(&root, &["checkout", "-q", "ticket/x"]);
        // plumbing, as a file named with a star or question mark cannot be written on every system
        for (name, id) in [("[ab].txt", &star), ("a.txt", &plain)] {
            let entry = format!("100644,{id},{name}");
            git_in(&root, &["update-index", "--add", "--cacheinfo", &entry]);
        }
        git_in(&root, &["commit", "-q", "-m", "glob"]);
        git_in(&root, &["checkout", "-q", "main"]);
        let patch = patch(&request(&root, None), "[ab].txt").await.unwrap();
        assert!(patch.patch.contains("+star"));
        assert!(!patch.patch.contains("+plain"));
        assert!(!patch.patch.contains("old.txt"));
    }

    #[tokio::test]
    async fn refuses_paths_outside_the_changes_and_option_like_names() {
        let (_dir, root) = repository();
        for path in [
            "only-main.txt",
            "--output=x",
            "nothing.txt",
            "../x",
            "/etc/passwd",
        ] {
            let error = patch(&request(&root, None), path).await.unwrap_err();
            assert_eq!(error.code(), "review-file-unknown", "{path}");
        }
        let mut bad = request(&root, None);
        bad.branch = String::from("--output=x");
        assert_eq!(
            changes(&bad).await.unwrap_err().code(),
            "review-branch-invalid"
        );
        let mut bad_target = request(&root, Some("--output=x"));
        bad_target.branch = String::from("ticket/x");
        assert_eq!(
            changes(&bad_target).await.unwrap_err().code(),
            "review-branch-invalid"
        );
    }

    #[tokio::test]
    async fn an_unknown_branch_is_a_git_failure() {
        let (_dir, root) = repository();
        let mut missing = request(&root, None);
        missing.branch = String::from("ticket/none");
        assert_eq!(changes(&missing).await.unwrap_err().code(), "git-failed");
    }

    fn root_text(root: &Path) -> String {
        root.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn shows_a_file_at_the_head_of_the_branch() {
        let (_dir, root) = repository();
        let shown = file(&root_text(&root), "ticket/x", "src/deep/file.rs")
            .await
            .unwrap();
        assert_eq!(shown.text.as_deref(), Some("fn main() {}\n"));
        assert_eq!(shown.size, 13);
        assert!(!shown.binary && !shown.truncated);
        // the working folder is not read: the branch head is
        fs::write(root.join("old.txt"), "changed on disk\n").unwrap();
        let old = file(&root_text(&root), "ticket/x", "old.txt")
            .await
            .unwrap();
        assert_eq!(old.text.as_deref(), Some("a\nnew\nc\n"));
    }

    #[tokio::test]
    async fn a_binary_file_has_no_text() {
        let (_dir, root) = repository();
        let shown = file(&root_text(&root), "ticket/x", "image.bin")
            .await
            .unwrap();
        assert!(shown.binary);
        assert_eq!(shown.text, None);
        assert_eq!(shown.size, 5);
    }

    #[tokio::test]
    async fn a_large_file_is_cut_at_the_limit() {
        let (_dir, root) = repository();
        git_in(&root, &["checkout", "-q", "ticket/x"]);
        let big = "x".repeat(MAX_FILE_BYTES + 10);
        commit(&root, &[("big.txt", big.as_bytes())], "big");
        git_in(&root, &["checkout", "-q", "main"]);
        let shown = file(&root_text(&root), "ticket/x", "big.txt")
            .await
            .unwrap();
        assert!(shown.truncated);
        assert_eq!(shown.size, (MAX_FILE_BYTES + 10) as u64);
        assert_eq!(shown.text.unwrap().len(), MAX_FILE_BYTES);
        let exact = "y".repeat(MAX_FILE_BYTES);
        git_in(&root, &["checkout", "-q", "ticket/x"]);
        commit(&root, &[("exact.txt", exact.as_bytes())], "exact");
        git_in(&root, &["checkout", "-q", "main"]);
        let shown = file(&root_text(&root), "ticket/x", "exact.txt")
            .await
            .unwrap();
        assert!(!shown.truncated);
    }

    #[tokio::test]
    async fn a_path_cannot_leave_the_repository_or_name_a_directory() {
        let (_dir, root) = repository();
        let outside = root.parent().unwrap().join("secret.txt");
        fs::write(&outside, "secret").unwrap();
        for path in [
            "../secret.txt",
            "src/../../secret.txt",
            "/etc/passwd",
            "src",
            "src/deep",
            "missing.txt",
            "",
        ] {
            let error = file(&root_text(&root), "ticket/x", path).await.unwrap_err();
            assert_eq!(error.code(), "review-file-unknown", "{path:?}");
        }
        let error = file(&root_text(&root), "--output=x", "old.txt")
            .await
            .unwrap_err();
        assert_eq!(error.code(), "review-branch-invalid");
    }

    #[test]
    fn cuts_text_on_a_character_boundary() {
        let text = "a\u{e4}b".as_bytes();
        assert_eq!(utf8_prefix(&text[..2]), "a");
        assert_eq!(utf8_prefix(text), "a\u{e4}b");
        assert_eq!(utf8_prefix(&[0xff, b'a']), "\u{fffd}a");
    }

    #[test]
    fn a_cut_list_drops_its_half_written_record() {
        let output = "M\0a.txt\0M\0b.tx";
        let list = name_status_of(output);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].path, "a.txt");
        let stats = numstat_of("1\t0\ta.txt\0");
        assert_eq!(stats["a.txt"].additions, 1);
    }
}
