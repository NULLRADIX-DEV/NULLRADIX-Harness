//! The login Claude Code keeps for an account, read from its own files.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use nullradix_app::secrets::SecretValue;
use nullradix_app::usage::{ClaudeLogin, ClaudeLogins, LoginRead};
use nullradix_domain::{ConfigDir, NetworkShares, Timestamp};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::accounts::{expand_home, refuses_typed};

/// Claude Code writes `.credentials.json` in place on Windows, so a reader may see half a file.
const UNREADABLE_RETRIES: u32 = 2;
const UNREADABLE_DELAY: Duration = Duration::from_millis(150);
const KEYCHAIN_BASE: &str = "Claude Code-credentials";

/// Reads logins from the files, and on macOS the keychain, of this machine.
#[derive(Debug)]
pub struct LocalClaudeLogins {
    home: Option<PathBuf>,
    /// `CLAUDE_CONFIG_DIR` of the daemon, which the default account follows.
    server_dir: Option<PathBuf>,
    delay: Duration,
    last: Mutex<HashMap<PathBuf, ClaudeLogin>>,
    shares: NetworkShares,
}

impl LocalClaudeLogins {
    /// From the daemon's own home folder and environment.
    #[must_use]
    pub fn from_environment() -> Self {
        let server_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from);
        Self::new(nullradix_os::home_dir(), server_dir)
    }

    #[must_use]
    pub fn new(home: Option<PathBuf>, server_dir: Option<PathBuf>) -> Self {
        Self {
            home,
            server_dir,
            delay: UNREADABLE_DELAY,
            last: Mutex::new(HashMap::new()),
            shares: NetworkShares::default(),
        }
    }

    /// Lets account folders on these hosts be read.
    #[must_use]
    pub fn with_network_shares(mut self, shares: NetworkShares) -> Self {
        self.shares = shares;
        self
    }

    pub(crate) fn refused(&self, dir: Option<&ConfigDir>) -> bool {
        dir.is_some_and(|dir| refuses_typed(&self.shares, dir.as_str()))
    }

    #[must_use]
    pub const fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// The folder typed for the account, else the server's, else `~/.claude`.
    fn dir_of(&self, dir: Option<&ConfigDir>) -> PathBuf {
        match dir {
            Some(dir) => expand_home(self.home.as_deref(), dir.as_str()),
            None => self
                .explicit(None)
                .unwrap_or_else(|| self.home.clone().unwrap_or_default().join(".claude")),
        }
    }

    /// The folder Claude Code was told about, none when it uses `~/.claude`.
    fn explicit(&self, dir: Option<&ConfigDir>) -> Option<PathBuf> {
        match dir {
            Some(dir) => Some(expand_home(self.home.as_deref(), dir.as_str())),
            None => self.server_dir.clone(),
        }
    }

    /// The file Claude Code keeps the login of the account in.
    pub(crate) fn credentials_file(&self, dir: Option<&ConfigDir>) -> PathBuf {
        self.dir_of(dir).join(".credentials.json")
    }

    fn identity_file(&self, dir: Option<&ConfigDir>) -> PathBuf {
        match self.explicit(dir) {
            Some(dir) => dir.join(".claude.json"),
            None => self.home.clone().unwrap_or_default().join(".claude.json"),
        }
    }

    #[cfg_attr(
        not(target_os = "macos"),
        allow(dead_code, reason = "only macOS keeps the login in the keychain")
    )]
    fn keychain_service(&self, dir: Option<&ConfigDir>) -> String {
        keychain_service(self.explicit(dir).as_deref())
    }

    fn remember(&self, file: PathBuf, login: &ClaudeLogin) {
        self.last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(file, login.clone());
    }

    fn remembered(&self, file: &Path) -> Option<ClaudeLogin> {
        self.last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(file)
            .cloned()
    }

    /// The raw login document: the keychain first on macOS, then the file.
    async fn document(&self, dir: Option<&ConfigDir>, file: PathBuf) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            let service = self.keychain_service(dir);
            let found = tokio::task::spawn_blocking(move || {
                nullradix_os::claude_keychain_password(&service)
            })
            .await
            .ok()
            .and_then(Result::ok)
            .flatten();
            if found.is_some() {
                return found;
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = dir;
        tokio::task::spawn_blocking(move || std::fs::read_to_string(file).ok())
            .await
            .ok()
            .flatten()
    }
}

/// The keychain item of Claude Code: a fixed name, or with 8 hex digits of the folder's SHA-256.
fn keychain_service(dir: Option<&Path>) -> String {
    let Some(dir) = dir else {
        return String::from(KEYCHAIN_BASE);
    };
    let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
    let hex = digest.iter().take(4).fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    });
    format!("{KEYCHAIN_BASE}-{hex}")
}

fn parse_login(document: &Value) -> Option<ClaudeLogin> {
    let oauth = document.get("claudeAiOauth")?;
    let token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())?;
    Some(ClaudeLogin {
        token: SecretValue::new(token),
        expires_at: oauth
            .get("expiresAt")
            .and_then(Value::as_i64)
            .map(Timestamp::from_millis),
        plan: oauth
            .get("subscriptionType")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

#[async_trait]
impl ClaudeLogins for LocalClaudeLogins {
    async fn read(&self, dir: Option<&ConfigDir>) -> LoginRead {
        if self.refused(dir) {
            return LoginRead::Missing;
        }
        let file = self.credentials_file(dir);
        for attempt in 0..=UNREADABLE_RETRIES {
            let Some(text) = self.document(dir, file.clone()).await else {
                return LoginRead::Missing;
            };
            let Ok(document) = serde_json::from_str::<Value>(&text) else {
                if attempt < UNREADABLE_RETRIES {
                    tokio::time::sleep(self.delay).await;
                }
                continue;
            };
            return match parse_login(&document) {
                Some(login) => {
                    self.remember(file, &login);
                    LoginRead::Found(login)
                }
                None => LoginRead::Missing,
            };
        }
        // half a file is no logout: the last good login still counts
        self.remembered(&file)
            .map_or(LoginRead::Unreadable, LoginRead::Found)
    }

    async fn email(&self, dir: Option<&ConfigDir>) -> Option<String> {
        if self.refused(dir) {
            return None;
        }
        let file = self.identity_file(dir);
        let text = tokio::task::spawn_blocking(move || std::fs::read_to_string(file).ok())
            .await
            .ok()
            .flatten()?;
        let document: Value = serde_json::from_str(&text).ok()?;
        document
            .get("oauthAccount")?
            .get("emailAddress")?
            .as_str()
            .filter(|email| !email.is_empty())
            .map(str::to_owned)
    }

    fn folder(&self, dir: Option<&ConfigDir>) -> String {
        self.dir_of(dir).to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGIN: &str = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-fake-access","refreshToken":"r","expiresAt":1900000000000,"subscriptionType":"max"}}"#;

    fn dir(path: &Path) -> ConfigDir {
        ConfigDir::parse(&path.to_string_lossy()).unwrap()
    }

    fn logins(home: &Path) -> LocalClaudeLogins {
        LocalClaudeLogins::new(Some(home.to_path_buf()), None).with_delay(Duration::from_millis(5))
    }

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }

    #[tokio::test]
    async fn reads_token_expiry_and_plan_of_an_account_folder() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("max");
        write(&folder, ".credentials.json", LOGIN);
        let read = logins(temp.path()).read(Some(&dir(&folder))).await;
        let LoginRead::Found(login) = read else {
            panic!("no login: {read:?}");
        };
        assert_eq!(login.token.expose(), "sk-ant-oat01-fake-access");
        assert_eq!(
            login.expires_at,
            Some(Timestamp::from_millis(1_900_000_000_000))
        );
        assert_eq!(login.plan.as_deref(), Some("max"));
    }

    #[tokio::test]
    async fn the_default_account_follows_the_server_folder_or_the_home_folder() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join(".claude"), ".credentials.json", LOGIN);
        let own = logins(temp.path());
        assert!(matches!(own.read(None).await, LoginRead::Found(_)));
        assert_eq!(
            own.folder(None),
            temp.path().join(".claude").to_string_lossy()
        );

        let server = temp.path().join("server");
        let following = LocalClaudeLogins::new(Some(temp.path().into()), Some(server.clone()));
        assert!(matches!(following.read(None).await, LoginRead::Missing));
        write(&server, ".credentials.json", LOGIN);
        assert!(matches!(following.read(None).await, LoginRead::Found(_)));
        assert_eq!(following.folder(None), server.to_string_lossy());
    }

    #[tokio::test]
    async fn a_tilde_folder_lives_below_the_home_folder() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join(".mcacc/work"), ".credentials.json", LOGIN);
        let typed = ConfigDir::parse("~/.mcacc/work").unwrap();
        let logins = logins(temp.path());
        assert!(matches!(
            logins.read(Some(&typed)).await,
            LoginRead::Found(_)
        ));
        assert_eq!(
            logins.folder(Some(&typed)),
            temp.path().join(".mcacc/work").to_string_lossy()
        );
    }

    #[tokio::test]
    async fn a_missing_file_or_a_file_without_a_login_is_no_login() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("max");
        let logins = logins(temp.path());
        assert_eq!(logins.read(Some(&dir(&folder))).await, LoginRead::Missing);
        write(&folder, ".credentials.json", r#"{"other":{}}"#);
        assert_eq!(logins.read(Some(&dir(&folder))).await, LoginRead::Missing);
        write(
            &folder,
            ".credentials.json",
            r#"{"claudeAiOauth":{"accessToken":""}}"#,
        );
        assert_eq!(logins.read(Some(&dir(&folder))).await, LoginRead::Missing);
    }

    #[tokio::test]
    async fn half_a_file_is_read_again_and_then_the_last_good_login_stands() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("max");
        let logins = logins(temp.path());
        write(&folder, ".credentials.json", r#"{"claudeAiOauth":{"acces"#);
        assert_eq!(
            logins.read(Some(&dir(&folder))).await,
            LoginRead::Unreadable
        );

        write(&folder, ".credentials.json", LOGIN);
        assert!(matches!(
            logins.read(Some(&dir(&folder))).await,
            LoginRead::Found(_)
        ));
        write(&folder, ".credentials.json", r#"{"claudeAiOauth":{"acces"#);
        let LoginRead::Found(login) = logins.read(Some(&dir(&folder))).await else {
            panic!("the last good login should stand");
        };
        assert_eq!(login.token.expose(), "sk-ant-oat01-fake-access");
    }

    #[tokio::test]
    async fn reads_the_email_of_the_account() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("max");
        let logins = logins(temp.path());
        assert_eq!(logins.email(Some(&dir(&folder))).await, None);
        write(
            &folder,
            ".claude.json",
            r#"{"oauthAccount":{"emailAddress":"max@example.com"}}"#,
        );
        assert_eq!(
            logins.email(Some(&dir(&folder))).await.as_deref(),
            Some("max@example.com")
        );
        write(
            &folder,
            ".claude.json",
            r#"{"oauthAccount":{"emailAddress":""}}"#,
        );
        assert_eq!(logins.email(Some(&dir(&folder))).await, None);
        write(&folder, ".claude.json", "not json");
        assert_eq!(logins.email(Some(&dir(&folder))).await, None);
    }

    #[tokio::test]
    async fn the_default_identity_sits_next_to_the_home_claude_folder() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path(),
            ".claude.json",
            r#"{"oauthAccount":{"emailAddress":"me@example.com"}}"#,
        );
        assert_eq!(
            logins(temp.path()).email(None).await.as_deref(),
            Some("me@example.com")
        );
        let server = temp.path().join("server");
        write(
            &server,
            ".claude.json",
            r#"{"oauthAccount":{"emailAddress":"srv@example.com"}}"#,
        );
        let following = LocalClaudeLogins::new(Some(temp.path().into()), Some(server));
        assert_eq!(
            following.email(None).await.as_deref(),
            Some("srv@example.com")
        );
    }

    #[test]
    fn the_keychain_item_is_named_like_claude_code_names_it() {
        assert_eq!(keychain_service(None), "Claude Code-credentials");
        assert_eq!(
            keychain_service(Some(Path::new("/home/ich/.mcacc/max/claude"))),
            "Claude Code-credentials-e3e67a67"
        );
    }

    #[test]
    fn tokens_never_show_in_debug_output() {
        let document: Value = serde_json::from_str(LOGIN).unwrap();
        let login = parse_login(&document).unwrap();
        assert!(!format!("{login:?}").contains("sk-ant-oat01-fake-access"));
    }

    #[tokio::test]
    async fn a_stored_network_folder_is_neither_read_nor_asked_for_its_email() {
        let temp = tempfile::tempdir().unwrap();
        let own = logins(temp.path());
        for text in [
            "//srv/share/claude",
            "\\\\srv\\share\\claude",
            "~/\\\\srv\\share",
        ] {
            let dir = ConfigDir::parse(text).unwrap();
            assert!(
                matches!(own.read(Some(&dir)).await, LoginRead::Missing),
                "{text}"
            );
            assert_eq!(own.email(Some(&dir)).await, None, "{text}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_listed_share_is_read_like_any_folder() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("max");
        write(&folder, ".credentials.json", LOGIN);
        write(
            &folder,
            ".claude.json",
            r#"{"oauthAccount":{"emailAddress":"me@example.com"}}"#,
        );
        // two leading slashes name a host to the check and the first folder to this system
        let doubled = ConfigDir::parse(&format!("/{}", folder.display())).unwrap();
        let host = doubled
            .as_str()
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap();
        let closed = logins(temp.path());
        assert!(matches!(
            closed.read(Some(&doubled)).await,
            LoginRead::Missing
        ));
        let listed = logins(temp.path()).with_network_shares(NetworkShares::new([host]));
        assert!(matches!(
            listed.read(Some(&doubled)).await,
            LoginRead::Found(_)
        ));
    }
}
