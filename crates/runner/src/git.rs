use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use nullradix_app::AppError;
use nullradix_app::repositories::{
    Located, network_path_refused, not_a_git_project, path_missing, path_relative,
};
use nullradix_domain::{NetworkShares, RunnerOs};

/// The repository the absolute folder `path` belongs to; worktrees resolve to their main checkout.
pub(crate) async fn locate(path: &str, shares: &NetworkShares) -> Result<Located, AppError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(path_missing());
    }
    // a stat or git call on a share already sends the credentials of the user
    if shares.refuses(path) {
        return Err(network_path_refused(path));
    }
    let folder = PathBuf::from(path);
    if !folder.is_absolute() {
        return Err(path_relative(path));
    }
    let fast = folder.clone();
    let known = shares.clone();
    let found = blocking(move || root_fast(&fast, &known))
        .await?
        .map_err(|refused| network_path_refused(&refused))?;
    let root = if let Some(root) = found {
        root
    } else {
        let common = common_dir_git(&folder)
            .await
            .ok_or_else(|| not_a_git_project(path))?;
        let known = shares.clone();
        blocking(move || main_root(Path::new(&common), &known))
            .await?
            .map_err(|refused| network_path_refused(&refused))?
    };
    let root = checked_root(root, shares)?;
    let canonical = root.clone();
    let key = blocking(move || local_key(Path::new(&canonical))).await?;
    let name = match root.rsplit('/').next() {
        Some(last) if !last.is_empty() => last.to_owned(),
        _ => root.clone(),
    };
    Ok(Located { root, key, name })
}

/// Turns away the first of `paths` that reaches an unlisted machine, before any file call.
pub(crate) fn refuse_networks(shares: &NetworkShares, paths: &[&str]) -> Result<(), AppError> {
    match paths.iter().find(|path| shares.refuses(path)) {
        Some(path) => Err(network_path_refused(path)),
        None => Ok(()),
    }
}

/// Refuses a resolved root on an unlisted share.
fn checked_root(root: String, shares: &NetworkShares) -> Result<String, AppError> {
    if shares.refuses(&root) {
        return Err(network_path_refused(&root));
    }
    Ok(root)
}

/// The text of a refused path.
type Refused = String;

/// The resolved path, `None` when missing, or the text of a refused one.
fn resolve(path: &Path, shares: &NetworkShares) -> Result<Option<PathBuf>, Refused> {
    let text = path.to_string_lossy();
    if shares.refuses(&text) {
        return Err(text.into_owned());
    }
    let Some(real) = real_path(path) else {
        return Ok(None);
    };
    let resolved = real.to_string_lossy();
    if shares.refuses(&resolved) {
        return Err(resolved.into_owned());
    }
    Ok(Some(real))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(AppError::internal)
}

/// Asks git for the common dir, as in `git rev-parse --git-common-dir`.
pub(crate) async fn common_dir_git(folder: &Path) -> Option<String> {
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
fn root_fast(folder: &Path, shares: &NetworkShares) -> Result<Option<String>, Refused> {
    if std::env::var_os("GIT_DIR").is_some() || std::env::var_os("GIT_COMMON_DIR").is_some() {
        return Ok(None);
    }
    let Some(start) = resolve(folder, shares)? else {
        return Ok(None);
    };
    if !start.is_absolute() {
        return Ok(None);
    }
    let mut current = start.as_path();
    loop {
        let git = current.join(".git");
        match fs::metadata(&git) {
            Err(_) => match current.parent() {
                Some(parent) => current = parent,
                None => return Ok(None),
            },
            Ok(meta) if meta.is_dir() => {
                return Ok(is_git_dir(&git).then(|| slashed(&current.to_string_lossy())));
            }
            Ok(_) => return linked_root(current, &git, shares),
        }
    }
}

/// Root behind a `.git` file: a worktree leads to its main checkout, a submodule is its own.
fn linked_root(
    folder: &Path,
    file: &Path,
    shares: &NetworkShares,
) -> Result<Option<String>, Refused> {
    let Ok(text) = fs::read_to_string(file) else {
        return Ok(None);
    };
    let Some(target) = text.trim().strip_prefix("gitdir:") else {
        return Ok(None);
    };
    let Some(git_dir) = resolve(&folder.join(target.trim()), shares)? else {
        return Ok(None);
    };
    match fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => {
            let Some(common) = resolve(&git_dir.join(common.trim()), shares)? else {
                return Ok(None);
            };
            if is_git_dir(&common) {
                main_root(&common, shares).map(Some)
            } else {
                Ok(None)
            }
        }
        Err(_) => Ok(is_git_dir(&git_dir).then(|| slashed(&folder.to_string_lossy()))),
    }
}

/// The main checkout of a common git dir: its `core.worktree` when set, else its parent.
fn main_root(common: &Path, shares: &NetworkShares) -> Result<String, Refused> {
    Ok(configured_worktree(common, shares)?.unwrap_or_else(|| root_of(&common.to_string_lossy())))
}

/// `core.worktree` from the common dir's config, which git sets for submodules.
fn configured_worktree(common: &Path, shares: &NetworkShares) -> Result<Option<String>, Refused> {
    let Ok(text) = fs::read_to_string(common.join("config")) else {
        return Ok(None);
    };
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
            let target = common.join(value.trim().trim_matches('"'));
            return Ok(resolve(&target, shares)?.map(|root| slashed(&root.to_string_lossy())));
        }
    }
    Ok(None)
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

/// `path_key` with the case rule of this system.
pub(crate) fn local_key(path: &Path) -> String {
    path_key(path, RunnerOs::current().ignores_path_case())
}

/// Comparable folder spelling: resolved, slashes, no trailing slash, lowercase if case is ignored.
pub(crate) fn path_key(path: &Path, ignore_case: bool) -> String {
    let full = real_path(path).unwrap_or_else(|| path.to_path_buf());
    let text = slashed(&full.to_string_lossy());
    let trimmed = text.trim_end_matches('/');
    let key = if trimmed.is_empty() || trimmed.ends_with(':') {
        text.as_str()
    } else {
        trimmed
    };
    if ignore_case {
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

    fn open() -> NetworkShares {
        NetworkShares::default()
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
        let located = run(locate(&text(&main), &open())).unwrap();
        assert_eq!(located.name, "Arches");
        assert!(located.root.ends_with("/Arches"), "{}", located.root);
        assert!(!located.root.contains('\\'));
        for folder in [main.join("src/deep"), dir.path().join("wt-1")] {
            let other = run(locate(&format!("  {}  ", text(&folder)), &open())).unwrap();
            assert_eq!(other, located, "{}", folder.display());
        }
        let common = run(common_dir_git(&dir.path().join("wt-1"))).unwrap();
        assert_eq!(
            local_key(Path::new(&main_root(Path::new(&common), &open()).unwrap())),
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
        let inner = run(locate(&text(&outer.join("lib")), &open())).unwrap();
        assert_eq!(inner.name, "lib");
        assert!(inner.root.ends_with("/outer/lib"), "{}", inner.root);
        let common = run(common_dir_git(&outer.join("lib"))).unwrap();
        assert_eq!(main_root(Path::new(&common), &open()).unwrap(), inner.root);
        assert_eq!(run(locate(&text(&outer), &open())).unwrap().name, "outer");
    }

    #[test]
    fn refuses_blank_relative_and_foreign_paths() {
        let dir = tempfile::tempdir().unwrap();
        let code = |path: &str| run(locate(path, &open())).unwrap_err().code();
        assert_eq!(code(" "), "path-missing");
        assert_eq!(code("relative/path"), "path-relative");
        assert_eq!(
            code(&text(&dir.path().join("missing"))),
            "not-a-git-project"
        );
        assert_eq!(root_fast(dir.path(), &open()), Ok(None));
    }

    #[test]
    fn network_and_device_paths_are_refused() {
        let code = |path: &str| run(locate(path, &open())).unwrap_err().code();
        for path in [
            "//server/share/repo",
            "\\\\server\\share\\repo",
            "  //Server/share",
            "\\\\?\\C:\\repo",
            "\\\\?\\UNC\\server\\share",
            "\\\\.\\pipe\\name",
            "///repo",
        ] {
            assert_eq!(code(path), "network-path-refused", "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_share_is_refused_before_the_folder_is_touched_unless_its_host_is_listed() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("Arches");
        repository(&main);
        // two leading slashes name a host to the check and the first folder to this system
        let doubled = format!("/{}", text(&main));
        let refused = run(locate(&doubled, &open())).unwrap_err();
        assert_eq!(refused.code(), "network-path-refused");
        let host = doubled.trim_start_matches('/').split('/').next().unwrap();
        let listed = NetworkShares::new([host.to_uppercase()]);
        assert_eq!(run(locate(&doubled, &listed)).unwrap().name, "Arches");
        let other = NetworkShares::new(["elsewhere"]);
        assert_eq!(
            run(locate(&doubled, &other)).unwrap_err().code(),
            "network-path-refused"
        );
    }

    /// `path` with one more leading slash, a host to the check and a folder to Unix.
    #[cfg(unix)]
    fn doubled(path: &Path) -> String {
        format!("/{}", text(path))
    }

    #[cfg(unix)]
    fn host_of(doubled: &str) -> String {
        doubled
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap()
            .to_owned()
    }

    #[cfg(unix)]
    fn linked_worktree(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let main = dir.join("Arches");
        repository(&main);
        git(&main, &["worktree", "add", "-q", "../wt-1"]);
        let tree = dir.join("wt-1");
        let git_dir = main.join(".git").join("worktrees").join("wt-1");
        (main, tree, git_dir)
    }

    #[cfg(unix)]
    #[test]
    fn a_git_file_that_leads_to_a_share_is_refused_unless_its_host_is_listed() {
        let dir = tempfile::tempdir().unwrap();
        let (_, tree, git_dir) = linked_worktree(dir.path());
        let shared = doubled(&git_dir);
        fs::write(tree.join(".git"), format!("gitdir: {shared}\n")).unwrap();
        let refused = run(locate(&text(&tree), &open())).unwrap_err();
        assert_eq!(refused.code(), "network-path-refused");
        assert!(refused.message().contains(&shared), "{}", refused.message());
        let listed = NetworkShares::new([host_of(&shared)]);
        assert_eq!(run(locate(&text(&tree), &listed)).unwrap().name, "Arches");
    }

    #[cfg(unix)]
    #[test]
    fn a_commondir_file_that_leads_to_a_share_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (main, tree, git_dir) = linked_worktree(dir.path());
        let shared = doubled(&main.join(".git"));
        fs::write(git_dir.join("commondir"), format!("{shared}\n")).unwrap();
        let refused = run(locate(&text(&tree), &open())).unwrap_err();
        assert_eq!(refused.code(), "network-path-refused");
        let listed = NetworkShares::new([host_of(&shared)]);
        assert_eq!(run(locate(&text(&tree), &listed)).unwrap().name, "Arches");
    }

    #[cfg(unix)]
    #[test]
    fn a_configured_worktree_that_leads_to_a_share_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (main, tree, _) = linked_worktree(dir.path());
        let shared = doubled(&main);
        let config = main.join(".git").join("config");
        let content = fs::read_to_string(&config).unwrap();
        fs::write(&config, format!("{content}[core]\n\tworktree = {shared}\n")).unwrap();
        let refused = run(locate(&text(&tree), &open())).unwrap_err();
        assert_eq!(refused.code(), "network-path-refused");
        let listed = NetworkShares::new([host_of(&shared)]);
        assert_eq!(run(locate(&text(&tree), &listed)).unwrap().name, "Arches");
    }

    #[test]
    fn a_root_that_resolves_to_a_share_is_refused_unless_its_host_is_listed() {
        let refused = checked_root(String::from("//srv/share/Arches"), &open()).unwrap_err();
        assert_eq!(refused.code(), "network-path-refused");
        assert!(refused.message().contains("//srv/share/Arches"));
        let listed = NetworkShares::new(["SRV"]);
        assert_eq!(
            checked_root(String::from("//srv/share/Arches"), &listed).unwrap(),
            "//srv/share/Arches"
        );
        assert_eq!(
            checked_root(String::from("C:/work/Arches"), &open()).unwrap(),
            "C:/work/Arches"
        );
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
