//! Claude folders of the accounts on this machine.

use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::accounts::{AccountFolders, dir_not_owned, folder_missing, folder_relative};
use nullradix_app::repositories::network_path_refused;
use nullradix_domain::{AccountSlug, ConfigDir, NetworkShares, RunnerOs};
use nullradix_os::create_data_dir;

use crate::git::local_key;

/// New accounts get a subfolder of `root`; folders typed by people may lie anywhere.
#[derive(Debug, Clone)]
pub struct LocalAccountFolders {
    root: PathBuf,
    home: Option<PathBuf>,
    shares: NetworkShares,
}

impl LocalAccountFolders {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            home: nullradix_os::home_dir(),
            shares: NetworkShares::default(),
        }
    }

    /// Lets folders on these hosts be used.
    #[must_use]
    pub fn with_network_shares(mut self, shares: NetworkShares) -> Self {
        self.shares = shares;
        self
    }

    #[must_use]
    pub fn with_home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }

    /// Config folders whose transcripts may be read: `base` and the folder of each account.
    #[must_use]
    pub fn transcript_roots(&self, base: Vec<PathBuf>, accounts: &[ConfigDir]) -> Vec<PathBuf> {
        let mut roots = base;
        for dir in accounts {
            if self.refused(dir.as_str()) {
                continue;
            }
            let path = self.expand(dir.as_str());
            if path.is_absolute() && !roots.contains(&path) {
                roots.push(path);
            }
        }
        roots
    }

    /// `~` and `~/...` stand for the home folder.
    fn expand(&self, typed: &str) -> PathBuf {
        expand_home(self.home.as_deref(), typed)
    }

    /// Whether the typed folder reaches an unlisted machine or a device.
    fn refused(&self, typed: &str) -> bool {
        refuses_typed(&self.shares, typed)
    }
}

/// Whether a typed folder reaches an unlisted machine or a device, also behind `~/`.
pub(crate) fn refuses_typed(shares: &NetworkShares, typed: &str) -> bool {
    // on Windows a rooted rest replaces the home folder
    let rest = typed
        .strip_prefix("~/")
        .or_else(|| typed.strip_prefix("~\\"));
    shares.refuses(typed) || rest.is_some_and(|rest| shares.refuses(rest))
}

/// `resolved_key` for a refused folder, from its text alone.
fn textual_key(typed: &str) -> String {
    let text = typed.trim().replace('\\', "/");
    let key = text.trim_end_matches('/');
    if RunnerOs::current().ignores_path_case() {
        key.to_lowercase()
    } else {
        key.to_owned()
    }
}

/// `~` and `~/...` stand for the home folder; anything else is taken as typed.
#[must_use]
pub fn expand_home(home: Option<&Path>, typed: &str) -> PathBuf {
    let rest = typed
        .strip_prefix("~/")
        .or_else(|| typed.strip_prefix("~\\"));
    match (home, typed, rest) {
        (Some(home), "~", _) => home.to_path_buf(),
        (Some(home), _, Some(rest)) => home.join(rest),
        _ => PathBuf::from(typed),
    }
}

/// `local_key` that also works below folders which do not exist yet or any more.
fn resolved_key(path: &Path) -> String {
    let mut head = path.to_path_buf();
    let mut tail: Vec<String> = Vec::new();
    while !head.exists() {
        let Some(name) = head
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            break;
        };
        tail.push(name);
        head.pop();
    }
    let base = local_key(&head);
    if tail.is_empty() {
        return base;
    }
    tail.reverse();
    let rest = tail.join("/");
    let rest = if RunnerOs::current().ignores_path_case() {
        rest.to_lowercase()
    } else {
        rest
    };
    format!("{}/{rest}", base.trim_end_matches('/'))
}

fn below_root(root_key: &str, key: &str) -> bool {
    key.strip_prefix(root_key)
        .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1)
}

#[async_trait]
impl AccountFolders for LocalAccountFolders {
    fn root(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }

    async fn create(&self, slug: &AccountSlug) -> Result<ConfigDir, AppError> {
        let path = self.root.join(slug.as_str());
        let made = path.clone();
        let root = self.root.clone();
        // private account folders, tightened when they are older
        tokio::task::spawn_blocking(move || {
            create_data_dir(&root)?;
            create_data_dir(&made)
        })
        .await
        .map_err(AppError::internal)?
        .map_err(AppError::internal)?;
        Ok(ConfigDir::from_stored(path.to_string_lossy().into_owned()))
    }

    async fn check(&self, dir: &ConfigDir) -> Result<(), AppError> {
        if self.refused(dir.as_str()) {
            return Err(network_path_refused(dir.as_str()));
        }
        let path = self.expand(dir.as_str());
        // a folder with a root but no drive is relative on Windows
        if !path.is_absolute() {
            return Err(folder_relative(dir));
        }
        let exists = tokio::task::spawn_blocking(move || path.is_dir())
            .await
            .map_err(AppError::internal)?;
        if exists {
            Ok(())
        } else {
            Err(folder_missing(dir))
        }
    }

    fn key(&self, dir: &ConfigDir) -> String {
        if self.refused(dir.as_str()) {
            return textual_key(dir.as_str());
        }
        resolved_key(&self.expand(dir.as_str()))
    }
    fn slug_key(&self, slug: &AccountSlug) -> String {
        resolved_key(&self.root.join(slug.as_str()))
    }

    fn owns(&self, dir: &ConfigDir) -> bool {
        if self.refused(dir.as_str()) {
            return false;
        }
        let path = self.expand(dir.as_str());
        let climbs = path
            .components()
            .any(|part| matches!(part, Component::ParentDir));
        !climbs && below_root(&resolved_key(&self.root), &resolved_key(&path))
    }

    async fn remove(&self, dir: &ConfigDir) -> Result<(), AppError> {
        if !self.owns(dir) {
            return Err(dir_not_owned(dir, &self.root()));
        }
        let path = self.expand(dir.as_str());
        tokio::task::spawn_blocking(move || remove_folder(&path))
            .await
            .map_err(AppError::internal)?
            .map_err(AppError::internal)
    }
}

fn remove_folder(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use nullradix_app::ErrorKind;

    use super::*;

    fn folders(dir: &Path) -> LocalAccountFolders {
        LocalAccountFolders::new(dir.join("accounts")).with_home(Some(dir.join("home")))
    }

    fn typed(path: &Path) -> ConfigDir {
        ConfigDir::parse(&path.to_string_lossy()).unwrap()
    }

    #[tokio::test]
    async fn creates_the_folder_of_a_new_account_below_the_root() {
        let temp = tempfile::tempdir().unwrap();
        let folders = folders(temp.path());
        let slug = AccountSlug::parse("max").unwrap();
        let dir = folders.create(&slug).await.unwrap();
        assert!(Path::new(dir.as_str()).is_dir());
        assert!(Path::new(dir.as_str()).ends_with("accounts/max"));
        folders.create(&slug).await.unwrap();
        assert!(folders.owns(&dir));
    }

    #[tokio::test]
    async fn typed_folders_must_be_absolute_and_exist() {
        let temp = tempfile::tempdir().unwrap();
        let folders = folders(temp.path());
        let relative = folders
            .check(&ConfigDir::parse("profiles/work").unwrap())
            .await
            .unwrap_err();
        assert_eq!(relative.code(), "account-invalid");
        assert_eq!(relative.kind(), ErrorKind::Invalid);
        let missing = folders
            .check(&typed(&temp.path().join("gone")))
            .await
            .unwrap_err();
        assert_eq!(missing.code(), "account-invalid");
        assert!(folders.check(&typed(temp.path())).await.is_ok());
        let file = temp.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(folders.check(&typed(&file)).await.is_err());
    }

    #[tokio::test]
    async fn a_tilde_stands_for_the_home_folder() {
        let temp = tempfile::tempdir().unwrap();
        let folders = folders(temp.path());
        std::fs::create_dir_all(temp.path().join("home/.mcacc/work")).unwrap();
        let tilde = ConfigDir::parse("~/.mcacc/work").unwrap();
        assert!(folders.check(&tilde).await.is_ok());
        assert_eq!(
            folders.key(&tilde),
            folders.key(&typed(&temp.path().join("home/.mcacc/work")))
        );
        assert!(folders.check(&ConfigDir::parse("~").unwrap()).await.is_ok());
        let nowhere = LocalAccountFolders::new(temp.path().join("accounts")).with_home(None);
        assert!(nowhere.check(&tilde).await.is_err());
    }

    #[test]
    fn transcripts_may_be_read_from_every_account_folder() {
        let temp = tempfile::tempdir().unwrap();
        let folders = folders(temp.path());
        let base = vec![temp.path().join("home/.claude")];
        let accounts = [
            typed(&temp.path().join("accounts/max")),
            ConfigDir::parse("~/.mcacc/work").unwrap(),
            typed(&temp.path().join("accounts/max")),
            ConfigDir::parse("relative/dir").unwrap(),
        ];
        let roots = folders.transcript_roots(base.clone(), &accounts);
        assert_eq!(
            roots,
            [
                base[0].clone(),
                temp.path().join("accounts/max"),
                temp.path().join("home/.mcacc/work"),
            ]
        );
        assert_eq!(folders.transcript_roots(base.clone(), &[]), base);
    }
    #[test]
    fn keys_ignore_trailing_slashes() {
        let temp = tempfile::tempdir().unwrap();
        let folders = folders(temp.path());
        let plain = typed(temp.path());
        let slashed = ConfigDir::parse(&format!("{}/", temp.path().display())).unwrap();
        assert_eq!(folders.key(&plain), folders.key(&slashed));
    }

    #[test]
    fn only_folders_below_the_root_are_owned() {
        let temp = tempfile::tempdir().unwrap();
        let folders = folders(temp.path());
        let root = temp.path().join("accounts");
        assert!(folders.owns(&typed(&root.join("max"))));
        assert!(folders.owns(&typed(&root.join("max").join("deeper"))));
        assert!(!folders.owns(&typed(&root)));
        assert!(!folders.owns(&typed(temp.path())));
        assert!(!folders.owns(&typed(&temp.path().join("accounts-other"))));
        assert!(!folders.owns(&typed(&root.join("..").join("home"))));
        assert!(!folders.owns(&typed(&root.join("max").join("..").join(".."))));
    }

    #[tokio::test]
    async fn removes_an_owned_folder_with_its_content_and_nothing_else() {
        let temp = tempfile::tempdir().unwrap();
        let folders = folders(temp.path());
        let slug = AccountSlug::parse("max").unwrap();
        let dir = folders.create(&slug).await.unwrap();
        std::fs::write(Path::new(dir.as_str()).join(".credentials.json"), "{}").unwrap();
        let other = temp.path().join("home");
        std::fs::create_dir_all(&other).unwrap();

        let refused = folders.remove(&typed(&other)).await.unwrap_err();
        assert_eq!(refused.code(), "account-invalid");
        assert!(other.is_dir());

        folders.remove(&dir).await.unwrap();
        assert!(!Path::new(dir.as_str()).exists());
        folders.remove(&dir).await.unwrap();
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn account_folders_are_private_and_older_open_ones_are_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let folders = folders(temp.path());
        let root = temp.path().join("accounts");
        let old = root.join("max");
        std::fs::create_dir_all(&old).unwrap();
        for dir in [&root, &old] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        folders
            .create(&AccountSlug::parse("max").unwrap())
            .await
            .unwrap();
        assert_eq!(mode_of(&root), 0o700);
        assert_eq!(mode_of(&old), 0o700);
        let fresh = folders
            .create(&AccountSlug::parse("work").unwrap())
            .await
            .unwrap();
        assert_eq!(mode_of(Path::new(fresh.as_str())), 0o700);
    }

    #[tokio::test]
    async fn network_and_device_folders_are_refused_without_touching_them() {
        let temp = tempfile::tempdir().unwrap();
        let folders = folders(temp.path());
        for text in [
            "//server/share/claude",
            "\\\\server\\share\\claude",
            "\\\\?\\C:\\claude",
            "\\\\.\\pipe\\claude",
            "~\\\\\\server\\share",
            "~/\\\\server\\share",
        ] {
            let dir = ConfigDir::parse(text).unwrap();
            let error = folders.check(&dir).await.unwrap_err();
            assert_eq!(error.code(), "network-path-refused", "{text}");
            assert_eq!(error.kind(), ErrorKind::Invalid);
            assert!(!folders.owns(&dir), "{text}");
            assert!(
                folders
                    .transcript_roots(Vec::new(), std::slice::from_ref(&dir))
                    .is_empty(),
                "{text}"
            );
        }
    }

    #[test]
    fn a_refused_folder_has_a_key_from_its_text_alone() {
        assert_eq!(textual_key(" \\\\srv\\x\\ "), "//srv/x");
        let temp = tempfile::tempdir().unwrap();
        let dir = ConfigDir::parse("\\\\srv\\x\\").unwrap();
        assert_eq!(folders(temp.path()).key(&dir), "//srv/x");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_share_is_refused_while_its_folder_exists_unless_its_host_is_listed() {
        let temp = tempfile::tempdir().unwrap();
        let existing = temp.path().join("claude");
        std::fs::create_dir_all(&existing).unwrap();
        // two leading slashes name a host to the check and the first folder to this system
        let doubled = ConfigDir::parse(&format!("/{}", existing.display())).unwrap();
        let closed = folders(temp.path());
        let refused = closed.check(&doubled).await.unwrap_err();
        assert_eq!(refused.code(), "network-path-refused");
        let host = doubled
            .as_str()
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap();
        let listed = folders(temp.path()).with_network_shares(NetworkShares::new([host]));
        assert!(listed.check(&doubled).await.is_ok());
        let other = folders(temp.path()).with_network_shares(NetworkShares::new(["elsewhere"]));
        assert!(other.check(&doubled).await.is_err());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_folder_without_a_drive_is_relative() {
        let temp = tempfile::tempdir().unwrap();
        let error = folders(temp.path())
            .check(&ConfigDir::parse("\\folder").unwrap())
            .await
            .unwrap_err();
        assert_eq!(error.code(), "account-invalid");
        assert!(error.message().contains("absolute"), "{}", error.message());
    }
}
