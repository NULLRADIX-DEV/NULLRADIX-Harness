//! Merges a ticket branch into its target with git on this machine, as legacy `BranchMerger` did.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::dispatch::merge_git::{
    BranchDeletion, DeleteBranch, MergeGit, MergeOutcome, MergeRequest, MergedEarlier,
};
use nullradix_app::review::check_branch;
use nullradix_app::worktrees::git_failed;
use nullradix_domain::{HoldKind, MergeStrategy, SQUASH_LIST_MAX, hold_files};

use crate::git::local_key;
use crate::worktree::{LONG_PATHS, attempt, ensure_target, exists, git, output};

/// Trailer of a squash commit: `Harness-Squashed: <branch> <full tip of the branch>`.
const SQUASH_TRAILER: &str = "Harness-Squashed";

/// Characters of git output a refusal carries.
const REFUSAL_MAX: usize = 2000;

/// Git states in which nothing is merged; a file in the git dir shows them.
const BUSY: [(&str, &str); 5] = [
    ("MERGE_HEAD", "a merge"),
    ("rebase-merge", "a rebase"),
    ("rebase-apply", "a rebase"),
    ("CHERRY_PICK_HEAD", "a cherry-pick"),
    ("REVERT_HEAD", "a revert"),
];

/// Merges with the git of this machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalMergeGit;

fn invalid(name: &str) -> AppError {
    AppError::invalid(
        "merge-branch-invalid",
        format!("\"{name}\" is not a branch name a merge can use"),
    )
}

/// A name git reads as a branch and never as an option or a range.
fn checked(name: &str) -> Result<(), AppError> {
    check_branch(name).map_err(|_| invalid(name))
}

/// What a git call answered, also when it failed.
struct Run {
    ok: bool,
    out: String,
    err: String,
}

async fn run(cwd: &Path, args: &[&str]) -> Result<Run, AppError> {
    let output = output(cwd, args).await?;
    Ok(Run {
        ok: output.status.success(),
        out: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        err: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

fn refs(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

async fn tip_of(cwd: &Path, branch: &str) -> Result<Option<String>, AppError> {
    let reference = format!("{}^{{commit}}", refs(branch));
    let found = attempt(cwd, &["rev-parse", "--verify", "--quiet", &reference]).await?;
    Ok(found.filter(|tip| !tip.is_empty()))
}

/// The folder that has the branch checked out, as git names it.
async fn checked_out_at(root: &Path, branch: &str) -> Result<Option<String>, AppError> {
    let listed = git(root, &["worktree", "list", "--porcelain"]).await?;
    let wanted = format!("branch {}", refs(branch));
    let mut current: Option<&str> = None;
    for line in listed.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(path) = line.strip_prefix("worktree ") {
            current = Some(path);
        } else if line == wanted {
            return Ok(current.map(str::to_owned));
        } else if line.is_empty() {
            current = None;
        }
    }
    Ok(None)
}

/// `<parent of root>/<root name>-worktrees/_merge/<target, "/" as "-">`.
fn merge_worktree_path(root: &Path, target: &str) -> String {
    let parent = root.parent().unwrap_or_else(|| Path::new(""));
    let base = root
        .file_name()
        .map_or_default(|name| name.to_string_lossy().into_owned());
    let path: PathBuf = parent
        .join(format!("{base}-worktrees"))
        .join("_merge")
        .join(target.replace('/', "-"));
    path.to_string_lossy().replace('\\', "/")
}

fn same_folder(left: &str, right: &str) -> bool {
    local_key(Path::new(left)) == local_key(Path::new(right))
}

/// Where the merge happens; a merge worktree opened for it is closed afterwards.
struct Place {
    path: String,
    into: String,
    opened: bool,
    /// The folder as a sentence names it.
    label: String,
}

/// Why the merge has no place, or the place.
enum Placed {
    Place(Place),
    Refused(String),
}

async fn place_of(root: &Path, target: Option<&str>) -> Result<Placed, AppError> {
    let Some(target) = target else {
        let current = git(root, &["branch", "--show-current"]).await?;
        if current.is_empty() {
            return Ok(Placed::Refused(String::from(
                "the main checkout is on no branch (detached HEAD)",
            )));
        }
        return Ok(Placed::Place(Place {
            path: slashed(root),
            into: current,
            opened: false,
            label: main_label(),
        }));
    };
    let at = match checked_out_at(root, target).await? {
        Some(path) if exists(PathBuf::from(&path)).await? => Some(path),
        // a listed folder that is gone counts as not checked out; prune clears it
        _ => None,
    };
    let own = merge_worktree_path(root, target);
    let main = slashed(root);
    match at {
        Some(at) if !same_folder(&at, &main) && !same_folder(&at, &own) => {
            Ok(Placed::Place(Place {
                label: format!("the worktree {at}"),
                path: at,
                into: target.to_owned(),
                opened: false,
            }))
        }
        Some(at) if same_folder(&at, &main) => Ok(Placed::Place(Place {
            path: main,
            into: target.to_owned(),
            opened: false,
            label: main_label(),
        })),
        _ => open_merge_worktree(root, target, &own).await,
    }
}

fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Reuses the merge worktree of the target, refuses a foreign folder, else adds it.
async fn open_merge_worktree(root: &Path, target: &str, path: &str) -> Result<Placed, AppError> {
    let place = |opened| {
        Placed::Place(Place {
            path: path.to_owned(),
            into: target.to_owned(),
            opened,
            label: format!("the merge worktree {path}"),
        })
    };
    if exists(PathBuf::from(path)).await? {
        let ours = checked_out_at(root, target)
            .await?
            .is_some_and(|at| same_folder(&at, path));
        if ours {
            return Ok(place(true));
        }
        return Err(AppError::invalid(
            "merge-worktree-foreign",
            format!(
                "the folder {path} exists already and is no worktree of branch {target}; check \
                 it and clean it up by hand"
            ),
        ));
    }
    git(root, &["worktree", "prune"]).await?;
    let mut args: Vec<&str> = LONG_PATHS.to_vec();
    args.extend(["worktree", "add", path, target]);
    git(root, &args).await?;
    Ok(place(true))
}

/// Names a git action that is under way in the folder, if any.
async fn running(cwd: &Path) -> Result<Option<&'static str>, AppError> {
    for (file, what) in BUSY {
        let Some(found) = attempt(cwd, &["rev-parse", "--git-path", file]).await? else {
            continue;
        };
        let path = PathBuf::from(&found);
        let full = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        if exists(full).await? {
            return Ok(Some(what));
        }
    }
    Ok(None)
}

fn main_label() -> String {
    String::from("the main checkout")
}

/// Files of the harness that never count as local changes.
const HARNESS_FILES: [&str; 3] = [
    ".claude/settings.local.json",
    ".claude/rules/ai-harness-docs.md",
    ".claude/rules/ai-harness-board.md",
];

/// Pointer of the old spec tooling; it only counts once it is staged.
const SPEC_POINTER: &str = ".specify/feature.json";

/// One line of `git status`: both status columns and the paths (a rename names both).
struct StatusEntry {
    columns: String,
    paths: Vec<String>,
}

async fn status_of(cwd: &Path) -> Result<Vec<StatusEntry>, AppError> {
    let args = [
        "-c",
        "core.quotePath=false",
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
    ];
    let output = output(cwd, &args).await?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(git_failed(&args, detail.trim()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = text.split('\0').collect();
    let mut entries = Vec::new();
    let mut at = 0;
    while at < parts.len() {
        let part = parts[at];
        at += 1;
        let (Some(columns), Some(path)) = (part.get(..2), part.get(3..)) else {
            continue;
        };
        let mut paths = vec![path.to_owned()];
        // a rename or copy is followed by the old path
        if columns.contains(['R', 'C']) {
            if let Some(old) = parts.get(at).filter(|old| !old.is_empty()) {
                paths.push((*old).to_owned());
            }
            at += 1;
        }
        entries.push(StatusEntry {
            columns: columns.to_owned(),
            paths,
        });
    }
    Ok(entries)
}

/// Local changes outside the harness files, sorted, with whether each is staged.
fn blocking(entries: &[StatusEntry]) -> Vec<(String, bool)> {
    let mut files: Vec<(String, bool)> = Vec::new();
    for entry in entries {
        let first = entry.columns.chars().next().unwrap_or(' ');
        let unstaged = first == ' ' || entry.columns == "??";
        let allowed = |file: &String| {
            HARNESS_FILES.contains(&file.as_str()) || (unstaged && file == SPEC_POINTER)
        };
        if !entry.paths.iter().all(allowed) {
            files.push((entry.paths[0].clone(), !unstaged));
        }
    }
    files.sort();
    files
}

/// Local changes that the branch changes too; none when git cannot tell.
async fn overlapping(cwd: &Path, tip: &str, entries: &[StatusEntry]) -> Vec<String> {
    let local: std::collections::BTreeSet<&str> = entries
        .iter()
        .flat_map(|entry| entry.paths.iter().map(String::as_str))
        .collect();
    let range = format!("HEAD...{tip}");
    let args = [
        "-c",
        "core.quotePath=false",
        "diff",
        "--name-only",
        "-z",
        "--no-renames",
        &range,
        "--",
    ];
    let Ok(output) = output(cwd, &args).await else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let files: std::collections::BTreeSet<String> = text
        .split('\0')
        .filter(|file| !file.is_empty() && local.contains(file))
        .map(str::to_owned)
        .collect();
    files.into_iter().collect()
}

/// Why git would refuse the merge in this folder now, though the folder will be fine later.
async fn hold_of(place: &Place, tip: &str) -> Result<Option<(HoldKind, String)>, AppError> {
    let cwd = Path::new(&place.path);
    if let Some(what) = running(cwd).await? {
        return Ok(Some((HoldKind::Busy, format!("{what} is under way"))));
    }
    let entries = status_of(cwd).await?;
    let local = blocking(&entries);
    if place.opened && !local.is_empty() {
        let files: Vec<String> = local.into_iter().map(|(file, _)| file).collect();
        return Ok(Some((
            HoldKind::Leftover,
            format!("changes of an earlier attempt in {}", hold_files(&files)),
        )));
    }
    let staged: Vec<String> = local
        .into_iter()
        .filter(|(_, staged)| *staged)
        .map(|(file, _)| file)
        .collect();
    if !staged.is_empty() {
        return Ok(Some((
            HoldKind::Staged,
            format!("staged changes (git add) in {}", hold_files(&staged)),
        )));
    }
    let overlap = overlapping(cwd, tip, &entries).await;
    Ok((!overlap.is_empty()).then(|| (HoldKind::Overlap, overlap_text(&overlap))))
}

fn overlap_text(files: &[String]) -> String {
    format!(
        "local changes in {} that the branch changes too",
        hold_files(files)
    )
}

async fn short_head(cwd: &Path) -> Result<String, AppError> {
    git(cwd, &["rev-parse", "--short", "HEAD"]).await
}

/// Whether a commit of `target` carries the squash trailer for the current tip of the branch.
async fn squashed_into(
    cwd: &Path,
    branch: &str,
    tip: &str,
    target: &str,
) -> Result<bool, AppError> {
    let grep = format!("--grep={SQUASH_TRAILER}: {branch} {tip}");
    let found = attempt(
        cwd,
        &[
            "log",
            "-1",
            "--format=%H",
            "--fixed-strings",
            &grep,
            target,
            "--",
        ],
    )
    .await?;
    Ok(found.is_some_and(|hash| !hash.is_empty()))
}

async fn is_ancestor(cwd: &Path, ancestor: &str, of: &str) -> Result<bool, AppError> {
    Ok(attempt(cwd, &["merge-base", "--is-ancestor", ancestor, of])
        .await?
        .is_some())
}

/// Merge or squash in `cwd`; the folder is left as it was unless the merge went through.
async fn merge_in(cwd: &Path, request: &MergeRequest, tip: &str) -> Result<MergeStep, AppError> {
    if let Some(what) = running(cwd).await? {
        return Ok(MergeStep::Refused(format!(
            "{what} is under way in {}; nothing is merged",
            cwd.display()
        )));
    }
    if squash_pending(cwd).await?
        && attempt(cwd, &["diff", "--cached", "--quiet"])
            .await?
            .is_none()
    {
        return Ok(MergeStep::Refused(format!(
            "an unfinished squash is left in {}; commit or reset its staged changes, nothing is              merged",
            cwd.display()
        )));
    }
    if squashed_into(cwd, &request.branch, tip, "HEAD").await? {
        return Ok(MergeStep::Merged(short_head(cwd).await?));
    }
    match request.strategy {
        MergeStrategy::Squash => squash(cwd, request, tip).await,
        MergeStrategy::Merge => {
            let args = [
                "-c",
                "core.quotePath=false",
                "merge",
                "--no-ff",
                "--no-edit",
                "-m",
                &request.message,
                tip,
            ];
            let tried = run(cwd, &args).await?;
            if tried.ok {
                return Ok(MergeStep::Merged(short_head(cwd).await?));
            }
            // nothing ran before, so a MERGE_HEAD now comes from this call
            let started = attempt(cwd, &["rev-parse", "--verify", "--quiet", "MERGE_HEAD"])
                .await?
                .is_some();
            after_failure(cwd, &tried, started, MergeStrategy::Merge).await
        }
    }
}

enum MergeStep {
    Merged(String),
    Conflict(Vec<String>),
    Refused(String),
    Held(HoldKind, String),
}

async fn squash(cwd: &Path, request: &MergeRequest, tip: &str) -> Result<MergeStep, AppError> {
    if is_ancestor(cwd, tip, "HEAD").await? {
        return Ok(MergeStep::Merged(short_head(cwd).await?));
    }
    // a squash would take staged changes into its commit
    if attempt(cwd, &["diff", "--cached", "--quiet"])
        .await?
        .is_none()
    {
        return Ok(MergeStep::Refused(String::from(
            "changes are staged in the checkout; a squash would take them along, commit them or \
             unstage them",
        )));
    }
    let paragraphs = squash_message(cwd, request, tip).await?;
    let pending_before = squash_pending(cwd).await?;
    let tried = run(
        cwd,
        &["-c", "core.quotePath=false", "merge", "--squash", tip],
    )
    .await?;
    if tried.ok {
        let mut commit = vec!["commit", "-q", "--allow-empty", "--no-verify"];
        for paragraph in &paragraphs {
            commit.extend(["-m", paragraph.as_str()]);
        }
        let done = run(cwd, &commit).await?;
        if done.ok {
            return Ok(MergeStep::Merged(short_head(cwd).await?));
        }
        abort(cwd, MergeStrategy::Squash).await?;
        // the reset keeps the message git wrote; the person's next commit must not open with it
        if let (false, Some(path)) = (pending_before, squash_msg_path(cwd).await?) {
            let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(path)).await;
        }
        let detail = [done.err, done.out]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(MergeStep::Refused(bounded(&format!(
            "the squash commit failed: {detail}"
        ))));
    }
    let unmerged = attempt(cwd, &["ls-files", "-u"]).await?;
    let started = (!pending_before && squash_pending(cwd).await?)
        || unmerged.is_some_and(|files| !files.is_empty());
    after_failure(cwd, &tried, started, MergeStrategy::Squash).await
}

/// Subject, the commits it sums up (oldest first, capped) and the trailer.
async fn squash_message(
    cwd: &Path,
    request: &MergeRequest,
    tip: &str,
) -> Result<Vec<String>, AppError> {
    let range = format!("HEAD..{tip}");
    let listed = git(cwd, &["log", "--reverse", "--format=- %s", &range, "--"]).await?;
    let subjects: Vec<&str> = listed.split('\n').filter(|line| !line.is_empty()).collect();
    let mut shown: Vec<String> = subjects
        .iter()
        .take(SQUASH_LIST_MAX)
        .map(|line| (*line).to_owned())
        .collect();
    if subjects.len() > shown.len() {
        shown.push(format!(
            "(... and {} more commits)",
            subjects.len() - shown.len()
        ));
    }
    let mut paragraphs = vec![request.message.clone()];
    if !shown.is_empty() {
        paragraphs.push(shown.join("\n"));
    }
    paragraphs.push(format!("{SQUASH_TRAILER}: {} {tip}", request.branch));
    Ok(paragraphs)
}

/// Where git keeps the message of a squash in this checkout.
async fn squash_msg_path(cwd: &Path) -> Result<Option<PathBuf>, AppError> {
    let Some(found) = attempt(cwd, &["rev-parse", "--git-path", "SQUASH_MSG"]).await? else {
        return Ok(None);
    };
    let path = PathBuf::from(&found);
    Ok(Some(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }))
}

async fn squash_pending(cwd: &Path) -> Result<bool, AppError> {
    match squash_msg_path(cwd).await? {
        Some(path) => exists(path).await,
        None => Ok(false),
    }
}

/// Lists the conflicted files, undoes our own attempt and tells conflict from refusal.
async fn after_failure(
    cwd: &Path,
    tried: &Run,
    started: bool,
    strategy: MergeStrategy,
) -> Result<MergeStep, AppError> {
    let listed = run(
        cwd,
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "--name-only",
            "--diff-filter=U",
        ],
    )
    .await?;
    let conflicted: Vec<String> = if listed.ok {
        listed
            .out
            .split('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    if started {
        abort(cwd, strategy).await?;
    }
    if !conflicted.is_empty() {
        return Ok(MergeStep::Conflict(conflicted));
    }
    let message = [tried.err.clone(), tried.out.clone()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    // a started merge without conflicts failed on a hook or a signature
    Ok(MergeStep::Refused(if message.is_empty() {
        String::from("git refused the merge")
    } else {
        bounded(&message)
    }))
}

/// The head of a git message, so a noisy hook cannot fill a comment.
fn bounded(text: &str) -> String {
    match text.char_indices().nth(REFUSAL_MAX) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_owned(),
    }
}

/// Undoes our own merge; when that fails the folder needs a person (`merge-abort-failed`).
async fn abort(cwd: &Path, strategy: MergeStrategy) -> Result<(), AppError> {
    let command: &[&str] = match strategy {
        MergeStrategy::Squash => &["reset", "-q", "--merge"],
        MergeStrategy::Merge => &["merge", "--abort"],
    };
    let aborted = run(cwd, command).await?;
    if aborted.ok {
        return Ok(());
    }
    let why = if aborted.err.is_empty() {
        aborted.out
    } else {
        aborted.err
    };
    let manual = command
        .iter()
        .filter(|argument| **argument != "-q")
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    Err(AppError::invalid(
        "merge-abort-failed",
        format!(
            "the folder {} is still in the merge state: {why}; clean it up by hand (git {manual})",
            cwd.display()
        ),
    ))
}

async fn close(root: &Path, path: &str) -> Option<String> {
    // without force: changes left behind stay
    match run(root, &["worktree", "remove", path]).await {
        Ok(closed) if closed.ok => None,
        Ok(closed) => Some(format!("merge worktree {path} stays: {}", closed.err)),
        Err(error) => Some(error.message().to_owned()),
    }
}

/// Checks the folder first, so a merge git would refuse waits instead of failing.
async fn step_in(place: &Place, request: &MergeRequest, tip: &str) -> Result<MergeStep, AppError> {
    if let Some((kind, text)) = hold_of(place, tip).await? {
        return Ok(MergeStep::Held(kind, text));
    }
    let cwd = Path::new(&place.path);
    let stepped = merge_in(cwd, request, tip).await?;
    if matches!(stepped, MergeStep::Refused(_)) {
        // git refused over local files the branch changes: that waits too
        let entries = status_of(cwd).await.unwrap_or_default();
        let files = overlapping(cwd, tip, &entries).await;
        if !files.is_empty() {
            return Ok(MergeStep::Held(HoldKind::Overlap, overlap_text(&files)));
        }
    }
    Ok(stepped)
}

/// Merges the captured tip of the branch, not the ref, which a commit may move meanwhile.
async fn merge_at(request: &MergeRequest, tip: String) -> Result<MergeOutcome, AppError> {
    let root = Path::new(&request.root);
    let place = match place_of(root, request.target.as_deref()).await? {
        Placed::Place(place) => place,
        Placed::Refused(reason) => return Ok(MergeOutcome::Refused { reason }),
    };
    let stepped = step_in(&place, request, &tip).await;
    // a merge worktree with leftovers cannot be removed; it stays until someone cleans up
    let leftover = matches!(stepped, Ok(MergeStep::Held(HoldKind::Leftover, _)));
    let warning = if place.opened && !leftover {
        close(root, &place.path).await
    } else {
        None
    };
    Ok(match stepped? {
        MergeStep::Merged(commit) => MergeOutcome::Merged {
            into: place.into,
            commit,
            tip,
            warning,
        },
        MergeStep::Conflict(files) => MergeOutcome::Conflict { files },
        MergeStep::Refused(reason) => MergeOutcome::Refused { reason },
        MergeStep::Held(kind, text) => {
            if let Some(warning) = warning {
                tracing::warn!("{warning}");
            }
            MergeOutcome::Held {
                kind,
                place: place.label,
                text,
            }
        }
    })
}

#[async_trait]
impl MergeGit for LocalMergeGit {
    async fn ensure_target(&self, root: &str, target: &str) -> Result<Option<String>, AppError> {
        checked(target)?;
        ensure_target(Path::new(root), target).await
    }

    async fn branch_tip(&self, root: &str, branch: &str) -> Result<Option<String>, AppError> {
        checked(branch)?;
        tip_of(Path::new(root), branch).await
    }

    async fn integrated(
        &self,
        root: &str,
        branch: &str,
        target: Option<&str>,
    ) -> Result<bool, AppError> {
        checked(branch)?;
        let root = Path::new(root);
        let Some(tip) = tip_of(root, branch).await? else {
            return Ok(false);
        };
        let into = match target {
            Some(target) => {
                checked(target)?;
                refs(target)
            }
            None => String::from("HEAD"),
        };
        Ok(is_ancestor(root, &tip, &into).await?
            || squashed_into(root, branch, &tip, &into).await?)
    }

    async fn merged_earlier(
        &self,
        root: &str,
        branch: &str,
        target: Option<&str>,
    ) -> Result<Option<MergedEarlier>, AppError> {
        checked(branch)?;
        let root = Path::new(root);
        if tip_of(root, branch).await?.is_some() {
            return Ok(None);
        }
        let into = match target {
            Some(target) => {
                checked(target)?;
                target.to_owned()
            }
            None => git(root, &["branch", "--show-current"]).await?,
        };
        if into.is_empty() {
            return Ok(None);
        }
        let format =
            format!("--format=%h%x1f%p%x1f%s%x1f%(trailers:key={SQUASH_TRAILER},valueonly)%x1e");
        let by_subject = format!("--grep=({branch})");
        let by_trailer = format!("--grep={SQUASH_TRAILER}: {branch} ");
        let into_ref = refs(&into);
        let found = attempt(
            root,
            &[
                "log",
                &format,
                "--fixed-strings",
                &by_subject,
                &by_trailer,
                &into_ref,
                "--",
            ],
        )
        .await?;
        let Some(found) = found else {
            return Ok(None);
        };
        for record in found.split('\u{1e}') {
            let fields: Vec<&str> = record.trim_start().split('\u{1f}').collect();
            let hash = fields.first().copied().unwrap_or_default();
            if hash.is_empty() {
                continue;
            }
            let parents = fields.get(1).copied().unwrap_or_default();
            let subject = fields.get(2).copied().unwrap_or_default();
            let trailers = fields.get(3).copied().unwrap_or_default();
            // a revert or a mention of the branch is no merge
            let is_merge = parents.split_whitespace().count() >= 2
                && subject.ends_with(&format!("({branch})"));
            let is_squash = trailers
                .lines()
                .any(|value| value.trim_start().starts_with(&format!("{branch} ")));
            if is_merge || is_squash {
                return Ok(Some(MergedEarlier {
                    into,
                    commit: hash.to_owned(),
                }));
            }
        }
        Ok(None)
    }

    async fn merge(&self, request: &MergeRequest) -> Result<MergeOutcome, AppError> {
        checked(&request.branch)?;
        if let Some(target) = &request.target {
            checked(target)?;
        }
        let root = Path::new(&request.root);
        let Some(tip) = tip_of(root, &request.branch).await? else {
            return Ok(MergeOutcome::Refused {
                reason: format!("branch {} does not exist", request.branch),
            });
        };
        merge_at(request, tip).await
    }

    async fn delete_merged(&self, request: &DeleteBranch) -> Result<BranchDeletion, AppError> {
        checked(&request.branch)?;
        checked(&request.into)?;
        let root = Path::new(&request.root);
        let branch = &request.branch;
        let Some(current) = tip_of(root, branch).await? else {
            return Ok(BranchDeletion::Deleted);
        };
        let kept = |why: String| {
            Ok(BranchDeletion::Kept(format!(
                "branch {branch} stays: {why}"
            )))
        };
        if current != request.tip {
            return kept(String::from("it has new commits since the merge"));
        }
        let into = refs(&request.into);
        let merged = is_ancestor(root, &request.tip, &into).await?
            || squashed_into(root, branch, &request.tip, &into).await?;
        if !merged {
            return kept(format!("it is not in {} yet", request.into));
        }
        let reference = refs(branch);
        let held = attempt(
            root,
            &["for-each-ref", "--format=%(worktreepath)", &reference],
        )
        .await?;
        if let Some(path) = held.filter(|path| !path.is_empty()) {
            return kept(format!("it is checked out in {path}"));
        }
        // the old value makes a commit that lands after the check keep the branch
        let deleted = run(root, &["update-ref", "-d", &reference, &request.tip]).await?;
        if !deleted.ok {
            let why = if deleted.err.is_empty() {
                String::from("it has new commits since the merge")
            } else {
                deleted.err
            };
            return kept(why);
        }
        attempt(
            root,
            &["config", "--remove-section", &format!("branch.{branch}")],
        )
        .await?;
        Ok(BranchDeletion::Deleted)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::{Command, Stdio};

    use super::*;
    use crate::worktree::has_ref;

    fn git_sync(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn commit(dir: &Path, file: &str, text: &str, message: &str) {
        fs::write(dir.join(file), text).unwrap();
        git_sync(dir, &["add", file]);
        git_sync(dir, &["commit", "-q", "-m", message]);
    }

    /// A repository on main with one commit and a ticket branch with one more.
    fn repository() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("arches");
        fs::create_dir_all(&root).unwrap();
        git_sync(&root, &["init", "-q", "-b", "main"]);
        // the merge itself commits with the repository's identity, which CI does not have
        git_sync(&root, &["config", "user.name", "t"]);
        git_sync(&root, &["config", "user.email", "t@example.com"]);
        git_sync(&root, &["config", "commit.gpgsign", "false"]);
        commit(&root, "a.txt", "base\n", "init");
        git_sync(&root, &["branch", "ticket/x"]);
        git_sync(&root, &["checkout", "-q", "ticket/x"]);
        commit(&root, "b.txt", "ticket\n", "on the ticket");
        git_sync(&root, &["checkout", "-q", "main"]);
        (dir, root)
    }

    fn request(root: &Path, target: Option<&str>, strategy: MergeStrategy) -> MergeRequest {
        MergeRequest {
            root: slashed(root),
            branch: String::from("ticket/x"),
            target: target.map(str::to_owned),
            message: String::from("Merge ARC-0001 \"Title\" (ticket/x)"),
            strategy,
        }
    }

    async fn merged(request: &MergeRequest) -> (String, String, String) {
        match LocalMergeGit.merge(request).await.unwrap() {
            MergeOutcome::Merged {
                into, commit, tip, ..
            } => (into, commit, tip),
            other => panic!("{other:?}"),
        }
    }

    fn conflicting(root: &Path) {
        git_sync(root, &["checkout", "-q", "ticket/x"]);
        commit(root, "a.txt", "ticket\n", "ticket line");
        git_sync(root, &["checkout", "-q", "main"]);
        commit(root, "a.txt", "main\n", "main line");
    }

    #[tokio::test]
    async fn merges_with_a_merge_commit_into_the_checked_out_branch() {
        let (_dir, root) = repository();
        let wanted = git_sync(&root, &["rev-parse", "refs/heads/ticket/x"]);
        let (into, commit, tip) = merged(&request(&root, None, MergeStrategy::Merge)).await;
        assert_eq!((into.as_str(), tip.as_str()), ("main", wanted.as_str()));
        assert_eq!(commit, git_sync(&root, &["rev-parse", "--short", "HEAD"]));
        assert_eq!(
            git_sync(&root, &["log", "-1", "--format=%s"]),
            "Merge ARC-0001 \"Title\" (ticket/x)"
        );
        assert_eq!(git_sync(&root, &["rev-parse", "HEAD^2"]), wanted);
        assert!(root.join("b.txt").exists());
    }

    #[tokio::test]
    async fn a_squash_is_one_commit_with_the_trailer_and_the_commit_list() {
        let (_dir, root) = repository();
        let before = git_sync(&root, &["rev-parse", "HEAD"]);
        let (_, _, tip) = merged(&request(&root, None, MergeStrategy::Squash)).await;
        assert_eq!(git_sync(&root, &["rev-parse", "HEAD^"]), before);
        let body = git_sync(&root, &["log", "-1", "--format=%B"]);
        assert!(body.starts_with("Merge ARC-0001"), "{body}");
        assert!(body.contains("- on the ticket"), "{body}");
        assert!(
            body.contains(&format!("Harness-Squashed: ticket/x {tip}")),
            "{body}"
        );
        // the same branch again adds nothing
        let again = merged(&request(&root, None, MergeStrategy::Squash)).await;
        assert_eq!(again.2, tip);
        assert_eq!(git_sync(&root, &["rev-parse", "HEAD^"]), before);
    }

    #[tokio::test]
    async fn a_failed_squash_commit_is_undone_without_its_message() {
        let (_dir, root) = repository();
        let head = git_sync(&root, &["rev-parse", "HEAD"]);
        // signing that cannot work makes the commit fail after the squash staged everything
        git_sync(&root, &["config", "commit.gpgsign", "true"]);
        git_sync(&root, &["config", "gpg.program", "nullradix-no-such-gpg"]);
        let outcome = LocalMergeGit
            .merge(&request(&root, None, MergeStrategy::Squash))
            .await
            .unwrap();
        assert!(
            matches!(&outcome, MergeOutcome::Refused { reason } if reason.starts_with("the squash commit failed")),
            "{outcome:?}"
        );
        assert_eq!(git_sync(&root, &["rev-parse", "HEAD"]), head);
        assert_eq!(git_sync(&root, &["status", "--porcelain"]), "");
        let message = git_sync(&root, &["rev-parse", "--git-path", "SQUASH_MSG"]);
        assert!(!root.join(message).exists());
    }

    #[tokio::test]
    async fn a_conflict_is_undone_and_names_its_files() {
        let (_dir, root) = repository();
        conflicting(&root);
        for strategy in [MergeStrategy::Merge, MergeStrategy::Squash] {
            let head = git_sync(&root, &["rev-parse", "HEAD"]);
            let outcome = LocalMergeGit
                .merge(&request(&root, None, strategy))
                .await
                .unwrap();
            assert_eq!(
                outcome,
                MergeOutcome::Conflict {
                    files: vec![String::from("a.txt")]
                }
            );
            assert_eq!(git_sync(&root, &["rev-parse", "HEAD"]), head);
            assert_eq!(git_sync(&root, &["status", "--porcelain"]), "");
            assert!(!root.join(".git/MERGE_HEAD").exists());
            assert!(!root.join(".git/SQUASH_MSG").exists());
        }
    }

    #[tokio::test]
    async fn a_merge_that_someone_else_started_holds_the_merge_and_is_left_alone() {
        let (_dir, root) = repository();
        conflicting(&root);
        let started = Command::new("git")
            .args(["merge", "ticket/x"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(!started.status.success());
        let outcome = LocalMergeGit
            .merge(&request(&root, None, MergeStrategy::Merge))
            .await
            .unwrap();
        assert!(
            matches!(&outcome, MergeOutcome::Held { kind: HoldKind::Busy, text, .. } if text.contains("a merge")),
            "{outcome:?}"
        );
        assert!(root.join(".git/MERGE_HEAD").exists());
    }

    fn hook_that_fails(root: &Path) {
        let hook = root.join(".git/hooks/pre-merge-commit");
        fs::create_dir_all(hook.parent().unwrap()).unwrap();
        fs::write(
            &hook,
            "#!/bin/sh
echo hook said no >&2
exit 1
",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[tokio::test]
    async fn a_merge_that_a_hook_stops_is_refused_with_the_hook_output_and_undone() {
        let (_dir, root) = repository();
        hook_that_fails(&root);
        let head = git_sync(&root, &["rev-parse", "HEAD"]);
        let outcome = LocalMergeGit
            .merge(&request(&root, None, MergeStrategy::Merge))
            .await
            .unwrap();
        assert!(
            matches!(&outcome, MergeOutcome::Refused { reason } if reason.contains("hook said no")),
            "{outcome:?}"
        );
        assert_eq!(git_sync(&root, &["rev-parse", "HEAD"]), head);
        assert_eq!(git_sync(&root, &["status", "--porcelain"]), "");
        assert!(!root.join(".git/MERGE_HEAD").exists());
    }

    #[test]
    fn a_refusal_keeps_only_the_head_of_long_output() {
        let long = "x".repeat(REFUSAL_MAX + 50);
        assert_eq!(bounded(&long).len(), REFUSAL_MAX + 3);
        assert_eq!(bounded("short"), "short");
    }

    #[tokio::test]
    async fn only_the_captured_tip_is_merged_when_the_branch_moves_meanwhile() {
        for strategy in [MergeStrategy::Merge, MergeStrategy::Squash] {
            let (_dir, root) = repository();
            let tip = tip_of(&root, "ticket/x").await.unwrap().unwrap();
            git_sync(&root, &["checkout", "-q", "ticket/x"]);
            commit(
                &root,
                "late.txt",
                "late
",
                "late commit",
            );
            git_sync(&root, &["checkout", "-q", "main"]);
            let outcome = merge_at(&request(&root, None, strategy), tip.clone())
                .await
                .unwrap();
            let MergeOutcome::Merged { tip: merged, .. } = outcome else {
                panic!("{outcome:?}");
            };
            assert_eq!(merged, tip);
            assert!(root.join("b.txt").exists());
            assert!(!root.join("late.txt").exists(), "{strategy:?}");
            let body = git_sync(&root, &["log", "-1", "--format=%B"]);
            assert!(!body.contains("late commit"), "{body}");
            assert!(body.contains("(ticket/x)"), "{body}");
            // the branch moved on, so the delete keeps it
            let kept = LocalMergeGit
                .delete_merged(&delete(&root, &tip))
                .await
                .unwrap();
            assert!(matches!(kept, BranchDeletion::Kept(_)), "{kept:?}");
            assert!(has_ref(&root, "refs/heads/ticket/x").await.unwrap());
        }
    }

    #[tokio::test]
    async fn an_unfinished_squash_in_the_checkout_holds_the_merge_for_both_strategies() {
        let (_dir, root) = repository();
        git_sync(&root, &["merge", "--squash", "ticket/x"]);
        assert!(root.join(".git/SQUASH_MSG").exists());
        for strategy in [MergeStrategy::Merge, MergeStrategy::Squash] {
            let outcome = LocalMergeGit
                .merge(&request(&root, None, strategy))
                .await
                .unwrap();
            assert!(
                matches!(&outcome, MergeOutcome::Held { kind: HoldKind::Staged, text, .. }
                    if text.contains("b.txt")),
                "{strategy:?}: {outcome:?}"
            );
            assert!(root.join(".git/SQUASH_MSG").exists());
            assert_eq!(git_sync(&root, &["status", "--porcelain"]), "A  b.txt");
        }
    }

    #[tokio::test]
    async fn a_target_that_is_not_checked_out_merges_in_its_own_worktree() {
        let (dir, root) = repository();
        git_sync(&root, &["branch", "release", "main"]);
        let (into, _, _) = merged(&request(&root, Some("release"), MergeStrategy::Merge)).await;
        assert_eq!(into, "release");
        assert!(!root.join("b.txt").exists());
        assert_eq!(git_sync(&root, &["show", "release:b.txt"]), "ticket");
        // the merge worktree is closed again
        assert!(!dir.path().join("arches-worktrees/_merge/release").exists());
        assert_eq!(
            git_sync(&root, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "main"
        );
    }

    #[tokio::test]
    async fn a_foreign_folder_at_the_merge_worktree_is_refused() {
        let (dir, root) = repository();
        git_sync(&root, &["branch", "release", "main"]);
        fs::create_dir_all(dir.path().join("arches-worktrees/_merge/release")).unwrap();
        let error = LocalMergeGit
            .merge(&request(&root, Some("release"), MergeStrategy::Merge))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "merge-worktree-foreign");
    }

    #[tokio::test]
    async fn a_detached_main_checkout_without_a_target_is_refused() {
        let (_dir, root) = repository();
        git_sync(&root, &["checkout", "-q", "--detach"]);
        let outcome = LocalMergeGit
            .merge(&request(&root, None, MergeStrategy::Merge))
            .await
            .unwrap();
        assert!(
            matches!(outcome, MergeOutcome::Refused { .. }),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn an_unsafe_branch_name_never_reaches_git() {
        let (_dir, root) = repository();
        let mut bad = request(&root, None, MergeStrategy::Merge);
        bad.branch = String::from("--upload-pack=x");
        assert_eq!(
            LocalMergeGit.merge(&bad).await.unwrap_err().code(),
            "merge-branch-invalid"
        );
        let target = request(&root, Some("-d"), MergeStrategy::Merge);
        assert_eq!(
            LocalMergeGit.merge(&target).await.unwrap_err().code(),
            "merge-branch-invalid"
        );
    }

    fn delete(root: &Path, tip: &str) -> DeleteBranch {
        DeleteBranch {
            root: slashed(root),
            branch: String::from("ticket/x"),
            tip: tip.to_owned(),
            into: String::from("main"),
        }
    }

    #[tokio::test]
    async fn the_branch_stays_when_it_moved_after_the_merge() {
        let (_dir, root) = repository();
        let (_, _, tip) = merged(&request(&root, None, MergeStrategy::Merge)).await;
        git_sync(&root, &["checkout", "-q", "ticket/x"]);
        commit(&root, "c.txt", "late\n", "late commit");
        git_sync(&root, &["checkout", "-q", "main"]);
        let kept = LocalMergeGit
            .delete_merged(&delete(&root, &tip))
            .await
            .unwrap();
        assert!(matches!(kept, BranchDeletion::Kept(_)), "{kept:?}");
        assert!(has_ref(&root, "refs/heads/ticket/x").await.unwrap());
        // at its new tip it is not merged either
        let late = git_sync(&root, &["rev-parse", "refs/heads/ticket/x"]);
        let unmerged = LocalMergeGit
            .delete_merged(&delete(&root, &late))
            .await
            .unwrap();
        assert!(
            matches!(&unmerged, BranchDeletion::Kept(why) if why.contains("not in main")),
            "{unmerged:?}"
        );
        assert!(has_ref(&root, "refs/heads/ticket/x").await.unwrap());
    }

    #[tokio::test]
    async fn a_merged_branch_is_deleted_unless_it_is_checked_out() {
        let (_dir, root) = repository();
        let (_, _, tip) = merged(&request(&root, None, MergeStrategy::Merge)).await;
        let folder = root.parent().unwrap().join("arches-worktrees/x");
        git_sync(&root, &["worktree", "add", &slashed(&folder), "ticket/x"]);
        let kept = LocalMergeGit
            .delete_merged(&delete(&root, &tip))
            .await
            .unwrap();
        assert!(
            matches!(&kept, BranchDeletion::Kept(why) if why.contains("checked out")),
            "{kept:?}"
        );
        git_sync(&root, &["worktree", "remove", &slashed(&folder)]);
        let gone = LocalMergeGit
            .delete_merged(&delete(&root, &tip))
            .await
            .unwrap();
        assert_eq!(gone, BranchDeletion::Deleted);
        assert!(!has_ref(&root, "refs/heads/ticket/x").await.unwrap());
    }

    #[tokio::test]
    async fn a_squashed_branch_counts_as_merged_for_the_delete() {
        let (_dir, root) = repository();
        let (_, _, tip) = merged(&request(&root, None, MergeStrategy::Squash)).await;
        assert!(
            LocalMergeGit
                .integrated(&slashed(&root), "ticket/x", None)
                .await
                .unwrap()
        );
        let gone = LocalMergeGit
            .delete_merged(&delete(&root, &tip))
            .await
            .unwrap();
        assert_eq!(gone, BranchDeletion::Deleted);
    }

    #[tokio::test]
    async fn merged_earlier_needs_a_merge_commit_or_the_trailer() {
        let (_dir, root) = repository();
        let path = slashed(&root);
        assert_eq!(
            LocalMergeGit
                .merged_earlier(&path, "ticket/x", None)
                .await
                .unwrap(),
            None,
            "the branch exists"
        );
        let (_, commit, _) = merged(&request(&root, None, MergeStrategy::Merge)).await;
        git_sync(&root, &["branch", "-D", "ticket/x"]);
        let found = LocalMergeGit
            .merged_earlier(&path, "ticket/x", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (found.into.as_str(), found.commit.as_str()),
            ("main", commit.as_str())
        );
        // a commit that only mentions the branch is no merge
        let (_other_dir, other) = repository();
        git_sync(
            &other,
            &["commit", "-q", "--allow-empty", "-m", "see (ticket/x)"],
        );
        git_sync(&other, &["branch", "-D", "ticket/x"]);
        assert_eq!(
            LocalMergeGit
                .merged_earlier(&slashed(&other), "ticket/x", None)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn merged_earlier_finds_a_squash_by_its_trailer() {
        let (_dir, root) = repository();
        merged(&request(&root, None, MergeStrategy::Squash)).await;
        git_sync(&root, &["branch", "-D", "ticket/x"]);
        let found = LocalMergeGit
            .merged_earlier(&slashed(&root), "ticket/x", None)
            .await
            .unwrap();
        assert!(found.is_some());
    }

    /// What a hold names, or a failure of the test.
    fn held(outcome: &MergeOutcome) -> (HoldKind, &str, &str) {
        match outcome {
            MergeOutcome::Held { kind, place, text } => (*kind, place, text),
            other => panic!("not held: {other:?}"),
        }
    }

    async fn attempt_merge(root: &Path, target: Option<&str>) -> MergeOutcome {
        LocalMergeGit
            .merge(&request(root, target, MergeStrategy::Merge))
            .await
            .unwrap()
    }

    /// The ticket branch also changes a.txt.
    fn ticket_changes_a(root: &Path) {
        git_sync(root, &["checkout", "-q", "ticket/x"]);
        commit(root, "a.txt", "base\nticket\n", "ticket line");
        git_sync(root, &["checkout", "-q", "main"]);
    }

    #[tokio::test]
    async fn a_dirty_file_the_branch_changes_holds_the_merge_until_it_is_clean() {
        for strategy in [MergeStrategy::Merge, MergeStrategy::Squash] {
            let (_dir, root) = repository();
            ticket_changes_a(&root);
            fs::write(root.join("a.txt"), "local\n").unwrap();
            let head = git_sync(&root, &["rev-parse", "HEAD"]);
            let outcome = LocalMergeGit
                .merge(&request(&root, None, strategy))
                .await
                .unwrap();
            let (kind, place, text) = held(&outcome);
            assert_eq!(kind, HoldKind::Overlap);
            assert_eq!(place, "the main checkout");
            assert_eq!(text, "local changes in a.txt that the branch changes too");
            assert_eq!(git_sync(&root, &["rev-parse", "HEAD"]), head);
            assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "local\n");
            git_sync(&root, &["checkout", "--", "a.txt"]);
            merged(&request(&root, None, strategy)).await;
            assert!(root.join("b.txt").exists());
        }
    }

    #[tokio::test]
    async fn an_untracked_file_in_the_way_holds_the_merge() {
        let (_dir, root) = repository();
        fs::write(root.join("b.txt"), "mine\n").unwrap();
        let outcome = attempt_merge(&root, None).await;
        assert_eq!(held(&outcome).0, HoldKind::Overlap);
        assert!(held(&outcome).2.contains("b.txt"), "{outcome:?}");
        assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "mine\n");
        fs::remove_file(root.join("b.txt")).unwrap();
        merged(&request(&root, None, MergeStrategy::Merge)).await;
        let text = fs::read_to_string(root.join("b.txt")).unwrap();
        assert_eq!(text.trim_end(), "ticket");
    }

    #[tokio::test]
    async fn local_changes_in_other_files_do_not_hold_the_merge() {
        let (_dir, root) = repository();
        fs::write(root.join("a.txt"), "local\n").unwrap();
        fs::write(root.join("other.txt"), "x\n").unwrap();
        merged(&request(&root, None, MergeStrategy::Merge)).await;
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "local\n");
        assert_eq!(
            git_sync(&root, &["status", "--porcelain"]),
            "M a.txt\n?? other.txt"
        );
    }

    #[tokio::test]
    async fn staged_changes_hold_the_merge_and_name_their_files() {
        let (_dir, root) = repository();
        fs::write(root.join("new.txt"), "x\n").unwrap();
        git_sync(&root, &["add", "new.txt"]);
        let outcome = attempt_merge(&root, None).await;
        let (kind, _, text) = held(&outcome);
        assert_eq!(kind, HoldKind::Staged);
        assert_eq!(text, "staged changes (git add) in new.txt");
        assert!(!root.join("b.txt").exists());
        git_sync(&root, &["commit", "-q", "-m", "local"]);
        merged(&request(&root, None, MergeStrategy::Merge)).await;
    }

    #[tokio::test]
    async fn files_of_the_harness_never_count_but_a_staged_spec_pointer_does() {
        let (_dir, root) = repository();
        fs::create_dir_all(root.join(".claude/rules")).unwrap();
        fs::create_dir_all(root.join(".specify")).unwrap();
        fs::write(root.join(".claude/settings.local.json"), "{}").unwrap();
        fs::write(root.join(".claude/rules/ai-harness-docs.md"), "x").unwrap();
        fs::write(root.join(".claude/rules/ai-harness-board.md"), "x").unwrap();
        fs::write(root.join(".specify/feature.json"), "{}").unwrap();
        git_sync(&root, &["add", ".specify/feature.json"]);
        let outcome = attempt_merge(&root, None).await;
        let (kind, _, text) = held(&outcome);
        assert_eq!(kind, HoldKind::Staged);
        assert_eq!(text, "staged changes (git add) in .specify/feature.json");
        git_sync(&root, &["reset", "-q", ".specify/feature.json"]);
        merged(&request(&root, None, MergeStrategy::Merge)).await;
    }

    #[tokio::test]
    async fn a_merge_worktree_with_leftovers_holds_and_stays_open() {
        let (dir, root) = repository();
        git_sync(&root, &["branch", "release", "main"]);
        let own = dir.path().join("arches-worktrees/_merge/release");
        git_sync(
            &root,
            &["worktree", "add", "-q", own.to_str().unwrap(), "release"],
        );
        fs::write(own.join("left.txt"), "x\n").unwrap();
        let outcome = attempt_merge(&root, Some("release")).await;
        let (kind, place, text) = held(&outcome);
        assert_eq!(kind, HoldKind::Leftover);
        assert!(place.starts_with("the merge worktree "), "{place}");
        assert_eq!(text, "changes of an earlier attempt in left.txt");
        assert!(own.join("left.txt").exists());
        fs::remove_file(own.join("left.txt")).unwrap();
        let (into, _, _) = merged(&request(&root, Some("release"), MergeStrategy::Merge)).await;
        assert_eq!(into, "release");
        assert!(!own.exists());
    }

    #[tokio::test]
    async fn a_busy_target_checked_out_elsewhere_holds_until_the_action_ends() {
        let (dir, root) = repository();
        git_sync(&root, &["branch", "release", "main"]);
        let elsewhere = dir.path().join("elsewhere");
        git_sync(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                elsewhere.to_str().unwrap(),
                "release",
            ],
        );
        let marker = git_sync(&elsewhere, &["rev-parse", "--git-path", "MERGE_HEAD"]);
        let marker = elsewhere.join(marker);
        fs::write(&marker, git_sync(&elsewhere, &["rev-parse", "HEAD"])).unwrap();
        let outcome = attempt_merge(&root, Some("release")).await;
        let (kind, place, text) = held(&outcome);
        assert_eq!(kind, HoldKind::Busy);
        assert!(place.starts_with("the worktree "), "{place}");
        assert_eq!(text, "a merge is under way");
        assert!(!dir.path().join("arches-worktrees/_merge/release").exists());
        fs::remove_file(&marker).unwrap();
        merged(&request(&root, Some("release"), MergeStrategy::Merge)).await;
        assert!(elsewhere.join("b.txt").exists());
    }
}
