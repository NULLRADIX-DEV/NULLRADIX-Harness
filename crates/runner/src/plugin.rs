//! Switches the plugin on in worktrees the runner creates, as init does for the project.

use std::path::Path;

use parking_lot::RwLock;

use crate::cleanup::blocking;
use crate::worktree::attempt;

static PLUGIN: RwLock<Option<String>> = RwLock::new(None);

/// Sets the plugin id worktrees get; `None` leaves them alone.
pub fn set_worktree_plugin(id: Option<String>) {
    *PLUGIN.write() = id;
}

/// Enables the configured plugin in the worktree; a failure is only a warning in the log.
pub(crate) async fn enable(worktree: &Path) {
    let Some(id) = PLUGIN.read().clone() else {
        return;
    };
    if let Err(error) = enable_with(worktree, &id).await {
        tracing::warn!(worktree = %worktree.display(), %error, "cannot enable the plugin in the worktree");
    }
}

async fn enable_with(worktree: &Path, id: &str) -> Result<(), String> {
    let common = attempt(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .await
    .map_err(|error| error.message().to_owned())?
    .ok_or_else(|| String::from("git named no common folder"))?;
    let (worktree, id) = (worktree.to_path_buf(), id.to_owned());
    blocking(move || {
        let line = format!("/{}", nullradix_os::LOCAL_SETTINGS);
        nullradix_os::add_git_excludes(Path::new(&common), &[line])
            .and_then(|_| nullradix_os::enable_plugin(&worktree, &id))
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.message().to_owned())?
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;
    use std::process::{Command, Stdio};

    use super::*;

    pub(crate) const ID: &str = "nullradix-dev@nullradix-dev-local";

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

    /// Every test sets the same id, so parallel tests agree on the global.
    pub(crate) fn configure() {
        set_worktree_plugin(Some(ID.to_owned()));
    }

    /// The switch is on, other keys stay and git sees nothing to commit.
    pub(crate) fn assert_enabled(worktree: &Path) {
        let text = fs::read_to_string(worktree.join(nullradix_os::LOCAL_SETTINGS)).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(settings["enabledPlugins"][ID], true);
        assert_eq!(git_sync(worktree, &["status", "--porcelain"]), "");
    }

    #[tokio::test]
    async fn keeps_other_keys_and_the_worktree_clean() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        git_sync(&root, &["init", "-q", "-b", "main"]);
        git_sync(&root, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let tree = dir.path().join("tree");
        git_sync(&root, &["worktree", "add", "-q", tree.to_str().unwrap()]);
        let file = tree.join(nullradix_os::LOCAL_SETTINGS);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, r#"{"permissions":{"allow":["Bash"]}}"#).unwrap();

        enable_with(&tree, ID).await.unwrap();
        enable_with(&tree, ID).await.unwrap();

        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(settings["permissions"]["allow"][0], "Bash");
        assert_enabled(&tree);
        let exclude = fs::read_to_string(root.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.matches("/.claude/settings.local.json").count(), 1);
    }

    #[tokio::test]
    async fn broken_settings_are_reported_and_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        git_sync(&root, &["init", "-q", "-b", "main"]);
        let file = root.join(nullradix_os::LOCAL_SETTINGS);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "{ not json").unwrap();

        let error = enable_with(&root, ID).await.unwrap_err();

        assert!(error.contains("not valid JSON"), "{error}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "{ not json");
    }
}
