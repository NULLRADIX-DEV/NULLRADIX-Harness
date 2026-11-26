//! Renews the login in `.credentials.json` as Claude Code does, for accounts with a run token.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_app::Clock;
use nullradix_app::usage::{LoginRenewal, Renewal};
use nullradix_domain::ConfigDir;
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::claude_login::LocalClaudeLogins;
use crate::curl;

/// Where Claude Code renews its login.
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
/// Client id of Claude Code.
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// Scopes asked for when the login names none.
const DEFAULT_SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const REQUEST_LIMIT: Duration = Duration::from_secs(10);

type Writer = dyn Fn(&Path, &[u8]) -> std::io::Result<()> + Send + Sync;

/// What the token endpoint answered; its body holds tokens and never shows in debug output.
#[derive(Clone)]
pub struct TokenReply {
    pub status: u16,
    pub body: Option<Value>,
}

impl std::fmt::Debug for TokenReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenReply")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// The token endpoint; tests stand in for it.
#[async_trait]
pub trait TokenEndpoint: Send + Sync {
    /// The error text never holds the request body, which carries the refresh token.
    async fn post(&self, body: &Value) -> Result<TokenReply, String>;
}

#[derive(Debug, Clone)]
pub struct CurlTokenEndpoint {
    url: String,
}

impl CurlTokenEndpoint {
    #[must_use]
    pub fn anthropic() -> Self {
        Self::at(TOKEN_URL)
    }

    #[must_use]
    pub fn at(url: &str) -> Self {
        Self {
            url: url.to_owned(),
        }
    }

    /// The request as a curl config, so the refresh token never appears on a command line.
    fn config(&self, body: &Value) -> Option<String> {
        // JSON text escapes control characters, so it fits a quoted config string
        let text = serde_json::to_string(body).ok()?;
        Some(format!(
            "url = \"{}\"\nrequest = \"POST\"\nheader = \"Content-Type: application/json\"\nheader = \"User-Agent: ai-harness\"\ndata = \"{}\"\n",
            curl::quoted(&self.url)?,
            curl::quoted(&text)?
        ))
    }
}

#[async_trait]
impl TokenEndpoint for CurlTokenEndpoint {
    async fn post(&self, body: &Value) -> Result<TokenReply, String> {
        let config = self
            .config(body)
            .ok_or_else(|| String::from("the request cannot be written"))?;
        let text = curl::run(&config, REQUEST_LIMIT).await?;
        let response = curl::split_response(&text)
            .ok_or_else(|| String::from("curl printed no HTTP response"))?;
        Ok(TokenReply {
            status: response.status,
            body: serde_json::from_str(response.body).ok(),
        })
    }
}

/// Renews logins in the files of this machine; never two at once for one file.
pub struct LocalLoginRenewal {
    logins: Arc<LocalClaudeLogins>,
    endpoint: Arc<dyn TokenEndpoint>,
    clock: Arc<dyn Clock>,
    /// The login lives in the keychain, which is not renewed here.
    keychain: bool,
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    /// Per file, the SHA-256 of the refresh token the endpoint refused, so it is not asked again.
    refused: Mutex<HashMap<PathBuf, Vec<u8>>>,
    /// Per file, a grant not yet written, with the SHA-256 of the refresh token it replaces.
    pending: Mutex<HashMap<PathBuf, (Vec<u8>, Grant)>>,
    write: Arc<Writer>,
}

impl std::fmt::Debug for LocalLoginRenewal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalLoginRenewal").finish_non_exhaustive()
    }
}

impl LocalLoginRenewal {
    #[must_use]
    pub fn new(
        logins: Arc<LocalClaudeLogins>,
        endpoint: Arc<dyn TokenEndpoint>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            logins,
            endpoint,
            clock,
            keychain: cfg!(target_os = "macos"),
            locks: Mutex::new(HashMap::new()),
            refused: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            write: Arc::new(nullradix_os::write_private),
        }
    }

    #[must_use]
    pub const fn with_keychain(mut self, keychain: bool) -> Self {
        self.keychain = keychain;
        self
    }

    fn lock_of(&self, file: &Path) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(self.locks.lock().entry(file.to_path_buf()).or_default())
    }

    async fn renew_file(&self, file: &Path) -> Renewal {
        let now = self.clock.now().as_millis();
        let document = match read_document(file).await {
            Document::Missing => return Renewal::NeedsLogin,
            Document::Unreadable => return Renewal::Failed,
            Document::Found(document) => document,
        };
        let Some(oauth) = document.get("claudeAiOauth") else {
            return Renewal::NeedsLogin;
        };
        // renewed by Claude Code, or by an earlier caller that held the lock
        if expires_at(oauth).is_some_and(|at| at > now) {
            return Renewal::Renewed;
        }
        let Some(refresh_token) = text_of(oauth, "refreshToken").map(str::to_owned) else {
            return Renewal::NeedsLogin;
        };
        let refresh_token = refresh_token.as_str();
        let digest = digest_of(refresh_token);
        if self.refused.lock().get(file) == Some(&digest) {
            return Renewal::NeedsLogin;
        }
        let pending = {
            let mut all = self.pending.lock();
            all.remove(file).filter(|(old, _)| *old == digest)
        };
        if let Some((_, grant)) = pending {
            let stale = grant.expires_at <= now;
            let stored = self.store(file, grant, refresh_token).await;
            // a kept grant that is already over is renewed with its own refresh token
            if stale && matches!(stored, Renewal::Renewed) {
                return Box::pin(self.renew_file(file)).await;
            }
            return stored;
        }
        let reply = self
            .endpoint
            .post(&json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": CLIENT_ID,
                "scope": scope_of(oauth),
            }))
            .await;
        let reply = match reply {
            Ok(reply) => reply,
            Err(error) => {
                tracing::debug!(error, "the token endpoint did not answer");
                return Renewal::Failed;
            }
        };
        if refused(&reply) {
            self.refused.lock().insert(file.to_path_buf(), digest);
            return Renewal::NeedsLogin;
        }
        let Some(grant) = reply
            .body
            .as_ref()
            .filter(|_| reply.status == 200)
            .and_then(|body| Grant::read(body, refresh_token, now))
        else {
            tracing::debug!(status = reply.status, "the token endpoint refused to renew");
            return Renewal::Failed;
        };
        self.refused.lock().remove(file);
        self.store(file, grant, refresh_token).await
    }

    /// The old refresh token is used up, so the new one must reach the file: on its latest state.
    async fn store(&self, file: &Path, grant: Grant, sent: &str) -> Renewal {
        let expires = grant.expires_at;
        let mut latest = match read_document(file).await {
            Document::Found(latest) => latest,
            Document::Missing => return Renewal::NeedsLogin,
            Document::Unreadable => return self.keep(file, grant, sent),
        };
        let Some(fields) = latest
            .get_mut("claudeAiOauth")
            .and_then(Value::as_object_mut)
        else {
            return Renewal::NeedsLogin;
        };
        match fields.get("refreshToken").and_then(Value::as_str) {
            // logged out meanwhile
            None => return Renewal::NeedsLogin,
            // another login replaced the refresh token we spent: it wins
            Some(token) if token != sent => return Renewal::Renewed,
            Some(_) => {}
        }
        // Claude Code renewed meanwhile and holds a later expiry: leave its login alone
        if fields
            .get("expiresAt")
            .and_then(Value::as_i64)
            .is_some_and(|at| at >= expires)
        {
            return Renewal::Renewed;
        }
        fields.insert("accessToken".into(), json!(grant.access_token));
        fields.insert("refreshToken".into(), json!(grant.refresh_token));
        fields.insert("expiresAt".into(), json!(expires));
        if let Some(scopes) = &grant.scopes {
            fields.insert("scopes".into(), json!(scopes));
        }
        let Ok(text) = serde_json::to_string(&latest) else {
            return Renewal::Failed;
        };
        let target = file.to_path_buf();
        let write = Arc::clone(&self.write);
        let written = tokio::task::spawn_blocking(move || write(&target, text.as_bytes())).await;
        if matches!(written, Ok(Ok(()))) {
            Renewal::Renewed
        } else {
            tracing::warn!(file = %file.display(), "the renewed login could not be written");
            self.keep(file, grant, sent)
        }
    }

    /// The spent token is gone from the endpoint: keep the grant for the next try.
    fn keep(&self, file: &Path, grant: Grant, sent: &str) -> Renewal {
        self.pending
            .lock()
            .insert(file.to_path_buf(), (digest_of(sent), grant));
        Renewal::Failed
    }
}

#[async_trait]
impl LoginRenewal for LocalLoginRenewal {
    async fn renew(&self, dir: Option<&ConfigDir>) -> Renewal {
        if self.keychain {
            return Renewal::NeedsLogin;
        }
        let file = self.logins.credentials_file(dir);
        let lock = self.lock_of(&file);
        let _renewing = lock.lock().await;
        self.renew_file(&file).await
    }

    async fn hold(&self, dir: Option<&ConfigDir>) -> Box<dyn Send> {
        let file = self.logins.credentials_file(dir);
        let held = self.lock_of(&file).lock_owned().await;
        self.pending.lock().remove(&file);
        self.refused.lock().remove(&file);
        Box::new(held)
    }
}

enum Document {
    Missing,
    /// Not readable JSON, which may be half a file Claude Code is writing.
    Unreadable,
    Found(Value),
}

async fn read_document(file: &Path) -> Document {
    let file = file.to_path_buf();
    let text = tokio::task::spawn_blocking(move || std::fs::read_to_string(file)).await;
    match text {
        Ok(Ok(text)) => serde_json::from_str(&text).map_or(Document::Unreadable, Document::Found),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Document::Missing,
        _ => Document::Unreadable,
    }
}

fn text_of<'a>(oauth: &'a Value, name: &str) -> Option<&'a str> {
    oauth
        .get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

fn expires_at(oauth: &Value) -> Option<i64> {
    oauth.get("expiresAt").and_then(Value::as_i64)
}

fn scope_of(oauth: &Value) -> String {
    let scopes = oauth
        .get("scopes")
        .and_then(Value::as_array)
        .map(|scopes| {
            scopes
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|scope| !scope.is_empty());
    scopes.unwrap_or_else(|| DEFAULT_SCOPES.to_owned())
}

fn digest_of(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// The endpoint says the refresh token is no good: it will not work on a retry either.
fn refused(reply: &TokenReply) -> bool {
    let error = reply
        .body
        .as_ref()
        .and_then(|body| body.get("error"))
        .and_then(Value::as_str);
    match reply.status {
        400 => error == Some("invalid_grant"),
        401 | 403 => matches!(
            error,
            Some("invalid_grant" | "invalid_token" | "unauthorized_client")
        ),
        _ => false,
    }
}

#[derive(Clone)]
struct Grant {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
    scopes: Option<Vec<String>>,
}

impl Grant {
    /// The renewed login of an answer; the refresh token stays when none comes back.
    fn read(body: &Value, old_refresh: &str, now: i64) -> Option<Self> {
        let access_token = text_of(body, "access_token")?.to_owned();
        let expires_in = body
            .get("expires_in")
            .and_then(Value::as_i64)
            .filter(|seconds| *seconds > 0)?;
        Some(Self {
            access_token,
            refresh_token: text_of(body, "refresh_token")
                .unwrap_or(old_refresh)
                .to_owned(),
            expires_at: now.saturating_add(expires_in.saturating_mul(1000)),
            scopes: body
                .get("scope")
                .and_then(Value::as_str)
                .map(|scope| scope.split_whitespace().map(str::to_owned).collect()),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
    use std::sync::mpsc;
    use std::thread;

    use tokio::sync::Notify;

    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> nullradix_domain::Timestamp {
            nullradix_domain::Timestamp::from_millis(NOW)
        }
    }

    struct MovingClock(Arc<AtomicI64>);

    impl Clock for MovingClock {
        fn now(&self) -> nullradix_domain::Timestamp {
            nullradix_domain::Timestamp::from_millis(self.0.load(Ordering::SeqCst))
        }
    }

    type Reply = Result<TokenReply, String>;

    /// Answers like the token endpoint and counts its asks; a gate holds the answer back.
    struct Endpoint {
        replies: Mutex<Vec<Reply>>,
        asked: AtomicU32,
        bodies: Mutex<Vec<Value>>,
        entered: Notify,
        gate: Option<Notify>,
        /// Runs while the answer is on its way, as Claude Code writing the file would.
        meanwhile: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    }

    impl Endpoint {
        fn new(replies: Vec<Reply>) -> Arc<Self> {
            Self::build(replies, false)
        }

        fn gated(replies: Vec<Reply>) -> Arc<Self> {
            Self::build(replies, true)
        }

        fn build(replies: Vec<Reply>, gated: bool) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(replies),
                asked: AtomicU32::new(0),
                bodies: Mutex::new(Vec::new()),
                entered: Notify::new(),
                gate: gated.then(Notify::new),
                meanwhile: Mutex::new(None),
            })
        }

        fn asks(&self) -> u32 {
            self.asked.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl TokenEndpoint for Endpoint {
        async fn post(&self, body: &Value) -> Result<TokenReply, String> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.bodies.lock().push(body.clone());
            self.entered.notify_one();
            if let Some(gate) = &self.gate {
                gate.notified().await;
            }
            let act = self.meanwhile.lock().take();
            if let Some(act) = act {
                act();
            }
            let mut replies = self.replies.lock();
            if replies.is_empty() {
                return Err(String::from("no reply"));
            }
            replies.remove(0)
        }
    }

    fn granted() -> Reply {
        answer(
            200,
            Some(json!({
                "access_token": "new-access",
                "refresh_token": "new-refresh",
                "expires_in": 28_800,
                "scope": "user:profile user:inference",
            })),
        )
    }

    #[allow(clippy::unnecessary_wraps, reason = "replies are queued as results")]
    fn answer(status: u16, body: Option<Value>) -> Reply {
        Ok(TokenReply { status, body })
    }

    const EXPIRED: &str = r#"{"claudeAiOauth":{"accessToken":"old-access","refreshToken":"old-refresh","expiresAt":1,"scopes":["user:profile"],"subscriptionType":"max","rateLimitTier":"x"},"mcpOAuth":{"keep":"me"}}"#;

    struct Rig {
        _temp: tempfile::TempDir,
        folder: PathBuf,
        renewal: LocalLoginRenewal,
        dir: ConfigDir,
    }

    fn rig(endpoint: Arc<Endpoint>, credentials: Option<&str>) -> Rig {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("max");
        std::fs::create_dir_all(&folder).unwrap();
        if let Some(text) = credentials {
            std::fs::write(folder.join(".credentials.json"), text).unwrap();
        }
        let logins = Arc::new(LocalClaudeLogins::new(Some(temp.path().into()), None));
        let renewal =
            LocalLoginRenewal::new(logins, endpoint, Arc::new(FixedClock)).with_keychain(false);
        let dir = ConfigDir::parse(&folder.to_string_lossy()).unwrap();
        Rig {
            _temp: temp,
            folder,
            renewal,
            dir,
        }
    }

    impl Rig {
        fn file(&self) -> PathBuf {
            self.folder.join(".credentials.json")
        }

        fn text(&self) -> String {
            std::fs::read_to_string(self.file()).unwrap()
        }

        fn json(&self) -> Value {
            serde_json::from_str(&self.text()).unwrap()
        }

        async fn renew(&self) -> Renewal {
            self.renewal.renew(Some(&self.dir)).await
        }
    }

    #[tokio::test]
    async fn an_expired_login_gets_new_tokens_and_keeps_every_other_field() {
        let endpoint = Endpoint::new(vec![granted()]);
        let rig = rig(endpoint.clone(), Some(EXPIRED));
        assert_eq!(rig.renew().await, Renewal::Renewed);
        let json = rig.json();
        let oauth = &json["claudeAiOauth"];
        assert_eq!(oauth["accessToken"], "new-access");
        assert_eq!(oauth["refreshToken"], "new-refresh");
        assert_eq!(oauth["expiresAt"], NOW + 28_800_000);
        assert_eq!(oauth["scopes"], json!(["user:profile", "user:inference"]));
        assert_eq!(oauth["subscriptionType"], "max");
        assert_eq!(oauth["rateLimitTier"], "x");
        assert_eq!(json["mcpOAuth"], json!({"keep": "me"}));
        let body = endpoint.bodies.lock()[0].clone();
        assert_eq!(
            body,
            json!({
                "grant_type": "refresh_token",
                "refresh_token": "old-refresh",
                "client_id": CLIENT_ID,
                "scope": "user:profile",
            })
        );
    }

    #[tokio::test]
    async fn the_old_refresh_token_stays_when_none_comes_back_and_default_scopes_are_asked() {
        let endpoint = Endpoint::new(vec![answer(
            200,
            Some(json!({"access_token": "a", "expires_in": 60})),
        )]);
        let rig = rig(
            endpoint.clone(),
            Some(r#"{"claudeAiOauth":{"accessToken":"o","refreshToken":"keep","expiresAt":5}}"#),
        );
        assert_eq!(rig.renew().await, Renewal::Renewed);
        let json = rig.json();
        let oauth = &json["claudeAiOauth"];
        assert_eq!(oauth["refreshToken"], "keep");
        assert!(oauth.get("scopes").is_none());
        assert_eq!(endpoint.bodies.lock()[0]["scope"], DEFAULT_SCOPES);
    }

    #[tokio::test]
    async fn a_login_that_is_still_valid_asks_nobody() {
        let endpoint = Endpoint::new(vec![]);
        let valid = EXPIRED.replace("\"expiresAt\":1", &format!("\"expiresAt\":{}", NOW + 1));
        let rig = rig(endpoint.clone(), Some(&valid));
        assert_eq!(rig.renew().await, Renewal::Renewed);
        assert_eq!(endpoint.asks(), 0);
        assert_eq!(rig.text(), valid);
    }

    #[tokio::test]
    async fn without_a_file_a_refresh_token_or_outside_the_file_nobody_is_asked() {
        let endpoint = Endpoint::new(vec![]);
        let missing = rig(endpoint.clone(), None);
        assert_eq!(missing.renew().await, Renewal::NeedsLogin);
        let bare = rig(endpoint.clone(), Some(r#"{"other":{}}"#));
        assert_eq!(bare.renew().await, Renewal::NeedsLogin);
        let no_token = rig(
            endpoint.clone(),
            Some(r#"{"claudeAiOauth":{"accessToken":"o","refreshToken":"","expiresAt":5}}"#),
        );
        assert_eq!(no_token.renew().await, Renewal::NeedsLogin);
        let keychain = rig(endpoint.clone(), Some(EXPIRED));
        let renewal = keychain.renewal.with_keychain(true);
        assert_eq!(
            renewal.renew(Some(&keychain.dir)).await,
            Renewal::NeedsLogin
        );
        assert_eq!(endpoint.asks(), 0);
    }

    #[tokio::test]
    async fn half_a_file_is_a_failure_that_may_pass_and_is_left_alone() {
        let endpoint = Endpoint::new(vec![]);
        let rig = rig(endpoint.clone(), Some(r#"{"claudeAiOauth":{"acc"#));
        assert_eq!(rig.renew().await, Renewal::Failed);
        assert_eq!(rig.text(), r#"{"claudeAiOauth":{"acc"#);
        assert_eq!(endpoint.asks(), 0);
    }

    #[tokio::test]
    async fn a_refused_refresh_token_needs_a_login_and_is_not_tried_again() {
        let endpoint = Endpoint::new(vec![
            answer(400, Some(json!({"error": "invalid_grant"}))),
            answer(401, Some(json!({"error": "invalid_token"}))),
            granted(),
        ]);
        let rig = rig(endpoint.clone(), Some(EXPIRED));
        assert_eq!(rig.renew().await, Renewal::NeedsLogin);
        assert_eq!(rig.renew().await, Renewal::NeedsLogin);
        assert_eq!(endpoint.asks(), 1);
        assert_eq!(rig.text(), EXPIRED);
        // a new login brings a new refresh token, which is worth asking with
        std::fs::write(rig.file(), EXPIRED.replace("old-refresh", "again")).unwrap();
        assert_eq!(rig.renew().await, Renewal::NeedsLogin);
        assert_eq!(endpoint.asks(), 2);
    }

    #[tokio::test]
    async fn failures_that_may_pass_leave_the_file_alone() {
        let endpoint = Endpoint::new(vec![
            Err(String::from("curl: could not connect")),
            answer(500, None),
            answer(429, Some(json!({"error": "rate_limited"}))),
            answer(400, Some(json!({"error": "invalid_request"}))),
            answer(200, Some(json!({"access_token": "a"}))),
            answer(200, Some(json!({"access_token": "", "expires_in": 5}))),
            answer(200, None),
        ]);
        let rig = rig(endpoint.clone(), Some(EXPIRED));
        for _ in 0..7 {
            assert_eq!(rig.renew().await, Renewal::Failed);
        }
        assert_eq!(rig.text(), EXPIRED);
        assert_eq!(endpoint.asks(), 7);
    }

    #[tokio::test]
    async fn a_login_that_cannot_be_written_back_is_a_failure() {
        let endpoint = Endpoint::new(vec![granted()]);
        let rig = rig(endpoint.clone(), Some(EXPIRED));
        // a folder in the place of the file cannot be replaced by the rename
        let file = rig.file();
        *endpoint.meanwhile.lock() = Some(Box::new(move || {
            std::fs::remove_file(&file).unwrap();
            std::fs::create_dir(&file).unwrap();
        }));
        assert_eq!(rig.renew().await, Renewal::Failed);
    }

    #[tokio::test]
    async fn claude_code_renewing_meanwhile_keeps_its_later_login() {
        let endpoint = Endpoint::new(vec![granted()]);
        let rig = rig(endpoint.clone(), Some(EXPIRED));
        let theirs = EXPIRED.replace("old-access", "their-access").replace(
            "\"expiresAt\":1",
            &format!("\"expiresAt\":{}", NOW + 50_000_000),
        );
        let file = rig.file();
        let written = theirs.clone();
        *endpoint.meanwhile.lock() = Some(Box::new(move || {
            std::fs::write(file, written).unwrap();
        }));
        assert_eq!(rig.renew().await, Renewal::Renewed);
        assert_eq!(rig.text(), theirs);
    }

    #[tokio::test]
    async fn fields_that_claude_code_changed_meanwhile_are_not_overwritten() {
        let endpoint = Endpoint::new(vec![granted()]);
        let rig = rig(endpoint.clone(), Some(EXPIRED));
        let file = rig.file();
        let theirs = EXPIRED.replace("\"keep\":\"me\"", "\"keep\":\"changed\"");
        *endpoint.meanwhile.lock() = Some(Box::new(move || {
            std::fs::write(file, theirs).unwrap();
        }));
        assert_eq!(rig.renew().await, Renewal::Renewed);
        let json = rig.json();
        assert_eq!(json["mcpOAuth"], json!({"keep": "changed"}));
        assert_eq!(json["claudeAiOauth"]["accessToken"], "new-access");
    }

    #[tokio::test]
    async fn another_login_that_replaced_the_refresh_token_is_left_alone() {
        let endpoint = Endpoint::new(vec![granted()]);
        let rig = rig(endpoint.clone(), Some(EXPIRED));
        let file = rig.file();
        let theirs = EXPIRED.replace("old-refresh", "their-refresh");
        let written = theirs.clone();
        *endpoint.meanwhile.lock() = Some(Box::new(move || {
            std::fs::write(file, written).unwrap();
        }));
        assert_eq!(rig.renew().await, Renewal::Renewed);
        assert_eq!(rig.text(), theirs);
    }

    #[tokio::test]
    async fn a_grant_that_could_not_be_written_is_written_before_the_endpoint_is_asked_again() {
        let endpoint = Endpoint::new(vec![granted()]);
        let mut rig = rig(endpoint.clone(), Some(EXPIRED));
        let failing = Arc::new(AtomicBool::new(true));
        rig.renewal.write = Arc::new({
            let failing = failing.clone();
            move |path, bytes| {
                if failing.swap(false, Ordering::SeqCst) {
                    return Err(std::io::Error::other("disk full"));
                }
                nullradix_os::write_private(path, bytes)
            }
        });
        assert_eq!(rig.renew().await, Renewal::Failed);
        assert_eq!(rig.text(), EXPIRED);
        assert_eq!(rig.renew().await, Renewal::Renewed);
        assert_eq!(endpoint.asks(), 1);
        let json = rig.json();
        assert_eq!(json["claudeAiOauth"]["accessToken"], "new-access");
        assert_eq!(json["claudeAiOauth"]["refreshToken"], "new-refresh");
    }

    #[tokio::test]
    async fn a_kept_grant_is_dropped_when_a_new_login_replaced_the_refresh_token() {
        let endpoint = Endpoint::new(vec![granted(), answer(500, None)]);
        let mut rig = rig(endpoint.clone(), Some(EXPIRED));
        rig.renewal.write = Arc::new(|_, _| Err(std::io::Error::other("disk full")));
        assert_eq!(rig.renew().await, Renewal::Failed);
        std::fs::write(rig.file(), EXPIRED.replace("old-refresh", "again")).unwrap();
        assert_eq!(rig.renew().await, Renewal::Failed);
        assert_eq!(endpoint.asks(), 2);
        assert_eq!(endpoint.bodies.lock()[1]["refresh_token"], "again");
    }

    #[tokio::test]
    async fn a_latest_file_that_cannot_be_read_keeps_the_grant_and_fails() {
        let endpoint = Endpoint::new(vec![granted()]);
        let rig = rig(endpoint.clone(), Some(EXPIRED));
        let file = rig.file();
        *endpoint.meanwhile.lock() = Some(Box::new(move || {
            std::fs::write(file, r#"{"claudeAiOauth":{"acc"#).unwrap();
        }));
        assert_eq!(rig.renew().await, Renewal::Failed);
        assert_eq!(rig.text(), r#"{"claudeAiOauth":{"acc"#);
        std::fs::write(rig.file(), EXPIRED).unwrap();
        assert_eq!(rig.renew().await, Renewal::Renewed);
        assert_eq!(endpoint.asks(), 1);
        assert_eq!(rig.json()["claudeAiOauth"]["refreshToken"], "new-refresh");
    }

    #[tokio::test]
    async fn a_logout_meanwhile_needs_a_login_and_is_not_written_over() {
        let endpoint = Endpoint::new(vec![granted()]);
        let rig = rig(endpoint.clone(), Some(EXPIRED));
        let file = rig.file();
        let loggedout = r#"{"claudeAiOauth":{"accessToken":"x","expiresAt":1},"mcpOAuth":{}}"#;
        *endpoint.meanwhile.lock() = Some(Box::new(move || {
            std::fs::write(file, loggedout).unwrap();
        }));
        assert_eq!(rig.renew().await, Renewal::NeedsLogin);
        assert_eq!(rig.text(), loggedout);
    }

    #[tokio::test]
    async fn a_kept_grant_that_is_already_over_is_renewed_at_once() {
        let second = answer(
            200,
            Some(json!({
                "access_token": "second-access",
                "refresh_token": "second-refresh",
                "expires_in": 28_800,
            })),
        );
        let endpoint = Endpoint::new(vec![granted(), second]);
        let mut rig = rig(endpoint.clone(), Some(EXPIRED));
        let clock = Arc::new(AtomicI64::new(NOW));
        rig.renewal.clock = Arc::new(MovingClock(clock.clone()));
        let failing = Arc::new(AtomicBool::new(true));
        rig.renewal.write = Arc::new({
            let failing = failing.clone();
            move |path, bytes| {
                if failing.swap(false, Ordering::SeqCst) {
                    return Err(std::io::Error::other("disk full"));
                }
                nullradix_os::write_private(path, bytes)
            }
        });
        assert_eq!(rig.renew().await, Renewal::Failed);
        clock.store(NOW + 30_000_000, Ordering::SeqCst);
        assert_eq!(rig.renew().await, Renewal::Renewed);
        assert_eq!(endpoint.asks(), 2);
        assert_eq!(endpoint.bodies.lock()[1]["refresh_token"], "new-refresh");
        assert_eq!(rig.json()["claudeAiOauth"]["accessToken"], "second-access");
    }

    #[tokio::test]
    async fn holding_a_folder_waits_for_a_renewal_and_drops_its_kept_grant() {
        let endpoint = Endpoint::new(vec![granted()]);
        let mut rig = rig(endpoint.clone(), Some(EXPIRED));
        rig.renewal.write = Arc::new(|_, _| Err(std::io::Error::other("disk full")));
        assert_eq!(rig.renew().await, Renewal::Failed);
        assert!(rig.renewal.pending.lock().contains_key(&rig.file()));
        let held = rig.renewal.hold(Some(&rig.dir)).await;
        assert!(rig.renewal.pending.lock().is_empty());
        let waiting = tokio::time::timeout(Duration::from_millis(50), rig.renew()).await;
        assert!(waiting.is_err(), "renewal ran while the folder was held");
        drop(held);
    }
    #[tokio::test]
    async fn a_403_that_is_not_an_oauth_refusal_may_pass() {
        let endpoint = Endpoint::new(vec![
            answer(403, None),
            answer(401, Some(json!({"error": "overloaded"}))),
            granted(),
        ]);
        let rig = rig(endpoint.clone(), Some(EXPIRED));
        assert_eq!(rig.renew().await, Renewal::Failed);
        assert_eq!(rig.renew().await, Renewal::Failed);
        assert_eq!(rig.renew().await, Renewal::Renewed);
        assert_eq!(endpoint.asks(), 3);
    }

    #[tokio::test]
    async fn callers_together_renew_once_per_file() {
        let endpoint = Endpoint::gated(vec![granted()]);
        let rig = Arc::new(rig(endpoint.clone(), Some(EXPIRED)));
        let first = tokio::spawn({
            let rig = rig.clone();
            async move { rig.renew().await }
        });
        endpoint.entered.notified().await;
        let second = tokio::spawn({
            let rig = rig.clone();
            async move { rig.renew().await }
        });
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert!(!second.is_finished(), "the second caller did not wait");
        endpoint.gate.as_ref().unwrap().notify_one();
        assert_eq!(first.await.unwrap(), Renewal::Renewed);
        assert_eq!(second.await.unwrap(), Renewal::Renewed);
        assert_eq!(endpoint.asks(), 1);
    }

    #[tokio::test]
    async fn different_files_renew_side_by_side() {
        let endpoint = Endpoint::gated(vec![granted(), granted()]);
        let one = Arc::new(rig(endpoint.clone(), Some(EXPIRED)));
        let two = tempfile::tempdir().unwrap();
        let other = two.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(".credentials.json"), EXPIRED).unwrap();
        let other_dir = ConfigDir::parse(&other.to_string_lossy()).unwrap();
        let first = tokio::spawn({
            let one = one.clone();
            async move { one.renew().await }
        });
        endpoint.entered.notified().await;
        let second = tokio::spawn({
            let one = one.clone();
            async move { one.renewal.renew(Some(&other_dir)).await }
        });
        endpoint.entered.notified().await;
        assert_eq!(endpoint.asks(), 2);
        endpoint.gate.as_ref().unwrap().notify_one();
        endpoint.gate.as_ref().unwrap().notify_one();
        assert_eq!(first.await.unwrap(), Renewal::Renewed);
        assert_eq!(second.await.unwrap(), Renewal::Renewed);
    }

    #[test]
    fn replies_never_show_tokens_in_debug_output() {
        let reply = granted().unwrap();
        assert!(!format!("{reply:?}").contains("new-access"));
    }

    /// Answers one request with `response` and reports the request text.
    fn serve(response: String) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/oauth/token", listener.local_addr().unwrap());
        let (sent, received) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                let read = stream.read(&mut chunk).unwrap();
                request.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&request).into_owned();
                let done = text.split_once("\r\n\r\n").is_some_and(|(head, body)| {
                    let length = head
                        .to_lowercase()
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:").map(str::to_owned))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    body.len() >= length
                });
                if read == 0 || done {
                    break;
                }
            }
            let _ = sent.send(String::from_utf8_lossy(&request).into_owned());
            let _ = stream.write_all(response.as_bytes());
        });
        (url, received)
    }

    fn http(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn the_curl_endpoint_posts_json_and_reads_the_answer() {
        let (url, request) = serve(http("200 OK", r#"{"access_token":"a","expires_in":9}"#));
        let body =
            json!({"grant_type": "refresh_token", "refresh_token": "with \"quote\" and \\ slash"});
        let reply = CurlTokenEndpoint::at(&url).post(&body).await.unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(
            reply.body,
            Some(json!({"access_token": "a", "expires_in": 9}))
        );
        let request = request.recv().unwrap();
        let (head, sent) = request.split_once("\r\n\r\n").unwrap();
        let head = head.to_lowercase();
        assert!(head.starts_with("post /v1/oauth/token"));
        assert!(head.contains("content-type: application/json"));
        assert!(head.contains("user-agent: ai-harness"));
        assert_eq!(serde_json::from_str::<Value>(sent).unwrap(), body);
    }

    #[tokio::test]
    async fn the_curl_endpoint_keeps_error_statuses_and_refuses_oversized_answers() {
        let (url, _) = serve(http("400 Bad Request", r#"{"error":"invalid_grant"}"#));
        let reply = CurlTokenEndpoint::at(&url).post(&json!({})).await.unwrap();
        assert_eq!(reply.status, 400);
        assert_eq!(reply.body, Some(json!({"error": "invalid_grant"})));

        let big = "x".repeat(curl::RESPONSE_MAX + 1);
        let (url, _) = serve(http("200 OK", &big));
        let error = CurlTokenEndpoint::at(&url)
            .post(&json!({}))
            .await
            .unwrap_err();
        assert!(error.contains("too large"), "{error}");
    }

    #[tokio::test]
    async fn an_unreachable_curl_endpoint_fails_without_the_request() {
        let address = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let error = CurlTokenEndpoint::at(&format!("http://{address}/token"))
            .post(&json!({"refresh_token": "secret-refresh"}))
            .await
            .unwrap_err();
        assert!(!error.contains("secret-refresh"));
    }
}
