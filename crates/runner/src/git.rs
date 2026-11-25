use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use nullradix_app::AppError;
use nullradix_app::repositories::{Located, not_a_git_project, path_missing, path_relative};

/// The repository the absolute folder `path` belongs to; worktrees resolve to their main checkout.
pub(crate) async fn locate(path: &str) -> Result<Located, AppError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(path_missing());
    }
    let folder = PathBuf::from(path);
    if !folder.is_absolute() {
        return Err(path_relative(path));
    }
    let fast = folder.clone();
    let root = if let Some(root) = blocking(move || root_fast(&fast)).await? {
        root
    } else {
        let common = common_dir_git(&folder)
            .await
            .ok_or_else(|| not_a_git_project(path))?;
        blocking(move || main_root(Path::new(&common))).await?
    };
    let canonical = root.clone();
    let key = blocking(move || path_key(Path::new(&canonical), cfg!(windows))).await?;
    let name = match root.rsplit('/').next() {
        Some(last) if !last.is_empty() => last.to_owned(),
        _ => root.clone(),
    };
    Ok(Located { root, key, name })
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(AppError::internal)
}

/// Asks git for the common dir, as in `git rev-parse --git-common-dir`.
async fn common_dir_git(folder: &Path) -> Option<String> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(folder)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().await.ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let common = text.trim();
    (output.status.success() && !common.is_empty()).then(|| slashed(common))
}

/// Main checkout root without starting git: `.git` upwards; `None` when only git can tell.
fn root_fast(folder: &Path) -> Option<String> {
    if std::env::var_os("GIT_DIR").is_some() || std::env::var_os("GIT_COMMON_DIR").is_some() {
        return None;
    }
    let start = real_path(folder)?;
    if !start.is_absolute() {
        return None;
    }
    let mut current = start.as_path();
    loop {
        let git = current.join(".git");
        match fs::metadata(&git) {
            Err(_) => current = current.parent()?,
            Ok(meta) if meta.is_dir() => {
                return is_git_dir(&git).then(|| slashed(&current.to_string_lossy()));
            }
            Ok(_) => return linked_root(current, &git),
        }
    }
}

/// Root behind a `.git` file: a worktree leads to its main checkout, a submodule is its own.
fn linked_root(folder: &Path, file: &Path) -> Option<String> {
    let text = fs::read_to_string(file).ok()?;
    let target = text.trim().strip_prefix("gitdir:")?.trim();
    let git_dir = real_path(&folder.join(target))?;
    match fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => {
            let common = real_path(&git_dir.join(common.trim()))?;
            is_git_dir(&common).then(|| main_root(&common))
        }
        Err(_) => is_git_dir(&git_dir).then(|| slashed(&folder.to_string_lossy())),
    }
}

/// The main checkout of a common git dir: its `core.worktree` when set, else its parent.
fn main_root(common: &Path) -> String {
    configured_worktree(common).unwrap_or_else(|| root_of(&common.to_string_lossy()))
}

/// `core.worktree` from the common dir's config, which git sets for submodules.
fn configured_worktree(common: &Path) -> Option<String> {
    let text = fs::read_to_string(common.join("config")).ok()?;
    let mut in_core = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_core = line.eq_ignore_ascii_case("[core]");
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if in_core && key.trim().eq_ignore_ascii_case("worktree") {
            let root = real_path(&common.join(value.trim().trim_matches('"')))?;
            return Some(slashed(&root.to_string_lossy()));
        }
    }
    None
}

fn is_git_dir(path: &Path) -> bool {
    path.join("HEAD").is_file() && path.join("objects").is_dir()
}

/// Resolved path in the spelling git uses, without the Windows `\\?\` prefix.
fn real_path(path: &Path) -> Option<PathBuf> {
    fs::canonicalize(path).ok().map(strip_verbatim)
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    let plain = if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{share}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        return path;
    };
    PathBuf::from(plain)
}

fn slashed(path: &str) -> String {
    path.replace('\\', "/")
}

/// Parent of the common dir, split on text like `dirname` in Node.
fn root_of(common_dir: &str) -> String {
    let normalized = slashed(common_dir);
    match normalized.rsplit_once('/') {
        Some(("", _)) => String::from("/"),
        Some((parent, _)) if parent.ends_with(':') => format!("{parent}/"),
        Some((parent, _)) => parent.to_owned(),
        None => String::from("."),
    }
}

/// Comparable folder spelling: resolved, forward slashes, no trailing slash, lowercase on Windows.
pub(crate) fn path_key(path: &Path, windows: bool) -> String {
    let full = real_path(path).unwrap_or_else(|| path.to_path_buf());
    let text = slashed(&full.to_string_lossy());
    let trimmed = text.trim_end_matches('/');
    let key = if trimmed.is_empty() || trimmed.ends_with(':') {
        text.as_str()
    } else {
        trimmed
    };
    if windows {
        key.to_lowercase()
    } else {
        key.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "protocol.file.allow=always"])
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn run<T>(work: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(work)
    }

    fn text(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    fn repository(path: &Path) {
        fs::create_dir_all(path).unwrap();
        git(path, &["init", "-q"]);
        git(path, &["commit", "-q", "--allow-empty", "-m", "init"]);
    }

    #[test]
    fn the_main_folder_a_subfolder_and_a_worktree_share_root_and_key() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("Arches");
        repository(&main);
        fs::create_dir_all(main.join("src/deep")).unwrap();
        git(&main, &["worktree", "add", "-q", "../wt-1"]);
        let located = run(locate(&text(&main))).unwrap();
        assert_eq!(located.name, "Arches");
        assert!(located.root.ends_with("/Arches"), "{}", located.root);
        assert!(!located.root.contains('\\'));
        for folder in [main.join("src/deep"), dir.path().join("wt-1")] {
            let other = run(locate(&format!("  {}  ", text(&folder)))).unwrap();
            assert_eq!(other, located, "{}", folder.display());
        }
        let common = run(common_dir_git(&dir.path().join("wt-1"))).unwrap();
        assert_eq!(
            path_key(Path::new(&main_root(Path::new(&common))), cfg!(windows)),
            located.key
        );
    }

    #[test]
    fn a_submodule_is_its_own_repository() {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("library");
        let outer = dir.path().join("outer");
        repository(&library);
        repository(&outer);
        git(&outer, &["submodule", "add", "-q", &text(&library), "lib"]);
        let inner = run(locate(&text(&outer.join("lib")))).unwrap();
        assert_eq!(inner.name, "lib");
        assert!(inner.root.ends_with("/outer/lib"), "{}", inner.root);
        let common = run(common_dir_git(&outer.join("lib"))).unwrap();
        assert_eq!(main_root(Path::new(&common)), inner.root);
        assert_eq!(run(locate(&text(&outer))).unwrap().name, "outer");
    }

    #[test]
    fn refuses_blank_relative_and_foreign_paths() {
        let dir = tempfile::tempdir().unwrap();
        let code = |path: &str| run(locate(path)).unwrap_err().code();
        assert_eq!(code(" "), "path-missing");
        assert_eq!(code("relative/path"), "path-relative");
        assert_eq!(
            code(&text(&dir.path().join("missing"))),
            "not-a-git-project"
        );
        assert_eq!(root_fast(dir.path()), None);
    }

    #[test]
    fn verbatim_paths_keep_their_drive_or_share() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\nas\share\Arches")),
            PathBuf::from(r"\\nas\share\Arches")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\Arches")),
            PathBuf::from(r"C:\Arches")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from("/home/x")),
            PathBuf::from("/home/x")
        );
    }

    #[test]
    fn roots_and_keys_follow_the_legacy_rules() {
        assert_eq!(root_of("/tmp/My Project/.git"), "/tmp/My Project");
        assert_eq!(root_of("C:\\Projects\\x\\.git"), "C:/Projects/x");
        assert_eq!(root_of("/.git"), "/");
        assert_eq!(root_of(".git"), ".");
        assert_eq!(
            path_key(Path::new("/missing-dir/Folder/"), false),
            "/missing-dir/Folder"
        );
        assert_eq!(
            path_key(Path::new("/missing-dir/Folder"), true),
            "/missing-dir/folder"
        );
    }
}
