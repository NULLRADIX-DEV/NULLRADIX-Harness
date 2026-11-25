//! Usage limits of the Claude subscription behind each account, as Claude Code shows them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_domain::{Account, ConfigDir, Id, Timestamp};
use parking_lot::Mutex;
use strum::IntoStaticStr;

use crate::secrets::SecretValue;
use crate::{AppError, Clock};

/// How long a successful reading stays current.
pub const SUCCESS_TTL: Duration = Duration::from_secs(60);
/// A failure counts only briefly, so a token that Claude renews right after is seen soon.
pub const FAILURE_TTL: Duration = Duration::from_secs(5);
/// Repeated failures of the endpoint double the wait up to this.
pub const MAX_FAILURE_TTL: Duration = Duration::from_secs(60);
/// After a 429 the endpoint is left alone at least this long.
pub const RATE_LIMIT_TTL: Duration = Duration::from_mins(5);

/// Why no numbers are known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
pub enum UsageReason {
    Disabled,
    NoLogin,
    TokenExpired,
    RequestFailed,
    RateLimited,
}

impl UsageReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// How much of one limit window is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageWindow {
    pub percent: u32,
    pub resets_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountUsage {
    /// None when the numbers are known.
    pub reason: Option<UsageReason>,
    pub plan: Option<String>,
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    pub fetched_at: Option<Timestamp>,
}

impl AccountUsage {
    #[must_use]
    pub fn unavailable(reason: UsageReason, plan: Option<String>) -> Self {
        Self {
            reason: Some(reason),
            plan,
            five_hour: None,
            seven_day: None,
            fetched_at: None,
        }
    }

    #[must_use]
    pub fn available(&self) -> bool {
        self.reason.is_none()
    }
}

/// The login Claude Code keeps for an account; its token is only ever used to ask for usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeLogin {
    pub token: SecretValue,
    pub expires_at: Option<Timestamp>,
    pub plan: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginRead {
    /// No login, or no file to read it from.
    Missing,
    /// The file never held readable JSON and nothing older is known.
    Unreadable,
    Found(ClaudeLogin),
}

/// Claude's own files of an account: its login and who it belongs to.
#[async_trait]
pub trait ClaudeLogins: Send + Sync {
    async fn read(&self, dir: Option<&ConfigDir>) -> LoginRead;
    /// The email of the logged in person.
    async fn email(&self, dir: Option<&ConfigDir>) -> Option<String>;
    /// The folder Claude Code uses for the account, with `~` and the server's own default resolved.
    fn folder(&self, dir: Option<&ConfigDir>) -> String;
}

/// What the usage endpoint answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageReply {
    pub status: u16,
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    /// The `Retry-After` header of a 429, seconds or an HTTP date.
    pub retry_after: Option<String>,
}

/// The undocumented endpoint behind Claude Code's `/usage`.
#[async_trait]
pub trait UsageApi: Send + Sync {
    async fn fetch(&self, token: &SecretValue) -> Result<UsageReply, AppError>;
}

/// An account with what is known about its login and limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStatus {
    pub account: Id<Account>,
    pub logged_in: bool,
    pub email: Option<String>,
    pub folder: String,
    pub usage: AccountUsage,
}

struct Cached {
    at: Timestamp,
    ttl: Duration,
    value: AccountUsage,
}

/// Readings and the failure streak of one account.
struct Memory {
    folder: String,
    cached: Mutex<Option<Cached>>,
    failures: Mutex<u32>,
    /// Callers that arrive together share one request: the second finds its answer cached.
    loading: tokio::sync::Mutex<()>,
}

struct Loaded {
    value: AccountUsage,
    ttl: Duration,
    /// A failure of the endpoint itself; repeating ones wait longer.
    backoff: bool,
}

pub struct UsageService {
    logins: Arc<dyn ClaudeLogins>,
    api: Arc<dyn UsageApi>,
    clock: Arc<dyn Clock>,
    memory: Mutex<HashMap<Id<Account>, Arc<Memory>>>,
}

impl std::fmt::Debug for UsageService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageService").finish_non_exhaustive()
    }
}

fn failed(reason: UsageReason, plan: Option<String>, ttl: Duration, backoff: bool) -> Loaded {
    Loaded {
        value: AccountUsage::unavailable(reason, plan),
        ttl,
        backoff,
    }
}

#[must_use]
pub fn request_failed(source: impl std::error::Error + Send + Sync + 'static) -> AppError {
    AppError::unavailable("usage-request-failed", "the usage endpoint did not answer")
        .with_source(source)
}

impl UsageService {
    #[must_use]
    pub fn new(
        logins: Arc<dyn ClaudeLogins>,
        api: Arc<dyn UsageApi>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            logins,
            api,
            clock,
            memory: Mutex::new(HashMap::new()),
        }
    }

    /// Login and limits of every account; disabled accounts show no numbers and ask nothing.
    pub async fn status(&self, accounts: &[Account], fresh: bool) -> Vec<AccountStatus> {
        futures_util::future::join_all(
            accounts
                .iter()
                .map(|account| self.account_status(account, fresh)),
        )
        .await
    }

    async fn account_status(&self, account: &Account, fresh: bool) -> AccountStatus {
        let dir = account.config_dir();
        let email = self.logins.email(dir).await;
        let usage = if account.enabled() {
            self.usage(account, fresh).await
        } else {
            AccountUsage::unavailable(UsageReason::Disabled, None)
        };
        // a disabled account is not asked, so only its email says it is logged in
        let logged_in =
            email.is_some() || (account.enabled() && usage.reason != Some(UsageReason::NoLogin));
        AccountStatus {
            account: account.id(),
            logged_in,
            email,
            folder: self.logins.folder(dir),
            usage,
        }
    }

    /// The limits of one account; `fresh` skips the cache unless a 429 locks the endpoint.
    pub async fn usage(&self, account: &Account, fresh: bool) -> AccountUsage {
        let memory = self.memory_of(account);
        if let Some(value) = self.current(&memory, fresh) {
            return value;
        }
        let _loading = memory.loading.lock().await;
        if let Some(value) = self.current(&memory, fresh) {
            return value;
        }
        let loaded = self.load(account.config_dir()).await;
        let ttl = Self::backoff(&memory, &loaded);
        *memory.cached.lock() = Some(Cached {
            at: self.clock.now(),
            ttl,
            value: loaded.value.clone(),
        });
        loaded.value
    }

    fn memory_of(&self, account: &Account) -> Arc<Memory> {
        let folder = self.logins.folder(account.config_dir());
        let mut all = self.memory.lock();
        if let Some(known) = all.get(&account.id())
            && known.folder == folder
        {
            return Arc::clone(known);
        }
        let memory = Arc::new(Memory {
            folder,
            cached: Mutex::new(None),
            failures: Mutex::new(0),
            loading: tokio::sync::Mutex::new(()),
        });
        all.insert(account.id(), Arc::clone(&memory));
        memory
    }

    fn current(&self, memory: &Memory, fresh: bool) -> Option<AccountUsage> {
        let cached = memory.cached.lock();
        let cached = cached.as_ref()?;
        if fresh && cached.value.reason != Some(UsageReason::RateLimited) {
            return None;
        }
        (self.clock.now().saturating_since(cached.at) < cached.ttl).then(|| cached.value.clone())
    }

    /// Success restarts the streak; each failure of the endpoint in a row doubles the wait.
    fn backoff(memory: &Memory, loaded: &Loaded) -> Duration {
        let mut failures = memory.failures.lock();
        if loaded.value.available() {
            *failures = 0;
            return loaded.ttl;
        }
        if !loaded.backoff {
            return loaded.ttl;
        }
        let ttl = loaded
            .ttl
            .saturating_mul(1_u32 << (*failures).min(10))
            .min(MAX_FAILURE_TTL);
        *failures += 1;
        ttl
    }

    async fn load(&self, dir: Option<&ConfigDir>) -> Loaded {
        let mut login = match self.logins.read(dir).await {
            LoginRead::Found(login) => login,
            LoginRead::Missing => return failed(UsageReason::NoLogin, None, FAILURE_TTL, false),
            LoginRead::Unreadable => {
                return failed(UsageReason::RequestFailed, None, FAILURE_TTL, false);
            }
        };
        if self.expired(&login) {
            match self.renewed(dir, &login.token).await {
                Some(renewed) => login = renewed,
                None => return failed(UsageReason::TokenExpired, login.plan, FAILURE_TTL, false),
            }
        }
        let rejected = |plan| failed(UsageReason::RequestFailed, plan, FAILURE_TTL, true);
        let Ok(mut reply) = self.api.fetch(&login.token).await else {
            return rejected(login.plan);
        };
        // another process renewed the token a moment ago: ask again with the new one
        if reply.status == 401
            && let Some(renewed) = self.renewed(dir, &login.token).await
        {
            login = renewed;
            let Ok(again) = self.api.fetch(&login.token).await else {
                return rejected(login.plan);
            };
            reply = again;
        }
        if reply.status == 429 {
            let ttl = self.lockout(reply.retry_after.as_deref());
            return failed(UsageReason::RateLimited, login.plan, ttl, false);
        }
        if reply.status >= 400 {
            let reason = if reply.status == 401 {
                UsageReason::TokenExpired
            } else {
                UsageReason::RequestFailed
            };
            return failed(reason, login.plan, FAILURE_TTL, true);
        }
        Loaded {
            value: AccountUsage {
                reason: None,
                plan: login.plan,
                five_hour: reply.five_hour,
                seven_day: reply.seven_day,
                fetched_at: Some(self.clock.now()),
            },
            ttl: SUCCESS_TTL,
            backoff: false,
        }
    }

    fn expired(&self, login: &ClaudeLogin) -> bool {
        login.expires_at.is_some_and(|at| at <= self.clock.now())
    }

    /// A usable login whose token differs from `old`, because someone else renewed it.
    async fn renewed(&self, dir: Option<&ConfigDir>, old: &SecretValue) -> Option<ClaudeLogin> {
        match self.logins.read(dir).await {
            LoginRead::Found(login) if login.token != *old && !self.expired(&login) => Some(login),
            _ => None,
        }
    }

    /// `Retry-After` in seconds or as an HTTP date, but never less than the standard lockout.
    fn lockout(&self, header: Option<&str>) -> Duration {
        let asked = header
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .and_then(|text| {
                text.parse::<f64>()
                    .ok()
                    .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                    .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
                    .or_else(|| {
                        Timestamp::parse_http_date(text)
                            .map(|at| at.saturating_since(self.clock.now()))
                    })
            });
        asked.map_or(RATE_LIMIT_TTL, |asked| asked.max(RATE_LIMIT_TTL))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicI64, Ordering};

    use nullradix_domain::{AccountName, AccountSlug};

    use super::*;

    struct TestClock(AtomicI64);

    impl TestClock {
        fn advance(&self, by: Duration) {
            self.0
                .fetch_add(i64::try_from(by.as_millis()).unwrap(), Ordering::SeqCst);
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> Timestamp {
            Timestamp::from_millis(self.0.load(Ordering::SeqCst))
        }
    }

    struct Logins {
        reads: Mutex<VecDeque<LoginRead>>,
        email: Option<String>,
    }

    #[async_trait]
    impl ClaudeLogins for Logins {
        async fn read(&self, _dir: Option<&ConfigDir>) -> LoginRead {
            let mut reads = self.reads.lock();
            if reads.len() > 1 {
                reads.pop_front().unwrap_or(LoginRead::Missing)
            } else {
                reads.front().cloned().unwrap_or(LoginRead::Missing)
            }
        }

        async fn email(&self, _dir: Option<&ConfigDir>) -> Option<String> {
            self.email.clone()
        }

        fn folder(&self, dir: Option<&ConfigDir>) -> String {
            dir.map_or_else(
                || String::from("/home/.claude"),
                |dir| dir.as_str().to_owned(),
            )
        }
    }

    struct Api {
        replies: Mutex<VecDeque<Result<UsageReply, AppError>>>,
        tokens: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl UsageApi for Api {
        async fn fetch(&self, token: &SecretValue) -> Result<UsageReply, AppError> {
            self.tokens.lock().push(token.expose().to_owned());
            self.replies
                .lock()
                .pop_front()
                .unwrap_or_else(|| Err(request_failed(std::io::Error::other("no reply"))))
        }
    }

    fn login(token: &str, expires_at: Option<i64>) -> LoginRead {
        LoginRead::Found(ClaudeLogin {
            token: SecretValue::new(token),
            expires_at: expires_at.map(Timestamp::from_millis),
            plan: Some(String::from("max")),
        })
    }

    #[allow(clippy::unnecessary_wraps, reason = "replies are queued as results")]
    fn reply(status: u16, percent: u32) -> Result<UsageReply, AppError> {
        Ok(UsageReply {
            status,
            five_hour: Some(UsageWindow {
                percent,
                resets_at: Some(Timestamp::from_millis(9)),
            }),
            seven_day: Some(UsageWindow {
                percent: percent / 2,
                resets_at: None,
            }),
            retry_after: None,
        })
    }

    #[allow(clippy::unnecessary_wraps, reason = "replies are queued as results")]
    fn limited(retry_after: Option<&str>) -> Result<UsageReply, AppError> {
        Ok(UsageReply {
            status: 429,
            five_hour: None,
            seven_day: None,
            retry_after: retry_after.map(str::to_owned),
        })
    }

    fn account(enabled: bool) -> Account {
        let account = Account::new(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            AccountSlug::parse("max").unwrap(),
            AccountName::parse("Max").unwrap(),
            ConfigDir::parse("/a/max").unwrap(),
            1,
            Timestamp::from_millis(0),
        );
        if enabled {
            account
        } else {
            account
                .changed(
                    nullradix_domain::AccountChanges {
                        enabled: Some(false),
                        ..nullradix_domain::AccountChanges::default()
                    },
                    Timestamp::from_millis(1),
                )
                .unwrap()
        }
    }

    struct Rig {
        service: UsageService,
        clock: Arc<TestClock>,
        api: Arc<Api>,
    }

    fn rig(reads: Vec<LoginRead>, replies: Vec<Result<UsageReply, AppError>>) -> Rig {
        let clock = Arc::new(TestClock(AtomicI64::new(1_000_000)));
        let api = Arc::new(Api {
            replies: Mutex::new(replies.into()),
            tokens: Mutex::new(Vec::new()),
        });
        let logins = Arc::new(Logins {
            reads: Mutex::new(reads.into()),
            email: None,
        });
        Rig {
            service: UsageService::new(logins, api.clone(), clock.clone()),
            clock,
            api,
        }
    }

    fn reason(usage: &AccountUsage) -> Option<&'static str> {
        usage.reason.map(UsageReason::as_str)
    }

    #[tokio::test]
    async fn reads_both_windows_and_the_plan() {
        let rig = rig(vec![login("t1", None)], vec![reply(200, 40)]);
        let usage = rig.service.usage(&account(true), false).await;
        assert!(usage.available());
        assert_eq!(usage.plan.as_deref(), Some("max"));
        assert_eq!(usage.five_hour.map(|window| window.percent), Some(40));
        assert_eq!(usage.seven_day.map(|window| window.percent), Some(20));
        assert_eq!(usage.fetched_at, Some(Timestamp::from_millis(1_000_000)));
        assert_eq!(*rig.api.tokens.lock(), ["t1"]);
    }

    #[tokio::test]
    async fn a_reading_is_reused_for_a_minute_unless_fresh_is_asked() {
        let rig = rig(
            vec![login("t1", None)],
            vec![reply(200, 10), reply(200, 20), reply(200, 30)],
        );
        let account = account(true);
        rig.service.usage(&account, false).await;
        rig.clock.advance(Duration::from_secs(59));
        let again = rig.service.usage(&account, false).await;
        assert_eq!(again.five_hour.map(|window| window.percent), Some(10));
        assert_eq!(rig.api.tokens.lock().len(), 1);
        let fresh = rig.service.usage(&account, true).await;
        assert_eq!(fresh.five_hour.map(|window| window.percent), Some(20));
        rig.clock.advance(Duration::from_secs(60));
        let later = rig.service.usage(&account, false).await;
        assert_eq!(later.five_hour.map(|window| window.percent), Some(30));
    }

    #[tokio::test]
    async fn no_login_and_unreadable_logins_are_short_lived_failures() {
        let rig = rig(vec![LoginRead::Missing], vec![]);
        let account = account(true);
        assert_eq!(
            reason(&rig.service.usage(&account, false).await),
            Some("no-login")
        );
        assert!(rig.api.tokens.lock().is_empty());
        let unreadable = self::rig(vec![LoginRead::Unreadable], vec![]);
        assert_eq!(
            reason(&unreadable.service.usage(&account, false).await),
            Some("request-failed")
        );
    }

    #[tokio::test]
    async fn an_expired_token_is_not_sent_but_a_renewed_one_is() {
        let rig = rig(vec![login("old", Some(900_000))], vec![]);
        let usage = rig.service.usage(&account(true), false).await;
        assert_eq!(reason(&usage), Some("token-expired"));
        assert_eq!(usage.plan.as_deref(), Some("max"));
        assert!(rig.api.tokens.lock().is_empty());

        let renewed = self::rig(
            vec![login("old", Some(900_000)), login("new", Some(9_000_000))],
            vec![reply(200, 5)],
        );
        let usage = renewed.service.usage(&account(true), false).await;
        assert!(usage.available());
        assert_eq!(*renewed.api.tokens.lock(), ["new"]);
    }

    #[tokio::test]
    async fn a_401_after_a_renewal_elsewhere_is_asked_again_with_the_new_token() {
        let rig = rig(
            vec![login("old", None), login("new", None)],
            vec![reply(401, 0), reply(200, 7)],
        );
        let usage = rig.service.usage(&account(true), false).await;
        assert!(usage.available());
        assert_eq!(*rig.api.tokens.lock(), ["old", "new"]);

        let unchanged = self::rig(vec![login("same", None)], vec![reply(401, 0)]);
        let usage = unchanged.service.usage(&account(true), false).await;
        assert_eq!(reason(&usage), Some("token-expired"));
        assert_eq!(unchanged.api.tokens.lock().len(), 1);
    }

    #[tokio::test]
    async fn failures_of_the_endpoint_wait_longer_each_time_up_to_a_minute() {
        let rig = rig(vec![login("t", None)], vec![]);
        let account = account(true);
        let mut waits = Vec::new();
        let first = rig.service.usage(&account, false).await;
        assert_eq!(reason(&first), Some("request-failed"));
        for _ in 0..6 {
            let before = rig.api.tokens.lock().len();
            let mut waited = 0;
            while rig.api.tokens.lock().len() == before {
                rig.clock.advance(Duration::from_secs(1));
                waited += 1;
                rig.service.usage(&account, false).await;
            }
            waits.push(waited);
        }
        assert_eq!(waits, [5, 10, 20, 40, 60, 60]);
    }

    #[tokio::test]
    async fn a_success_ends_the_failure_streak() {
        let rig = rig(
            vec![login("t", None)],
            vec![
                Ok(UsageReply {
                    status: 500,
                    five_hour: None,
                    seven_day: None,
                    retry_after: None,
                }),
                reply(200, 1),
                Ok(UsageReply {
                    status: 503,
                    five_hour: None,
                    seven_day: None,
                    retry_after: None,
                }),
            ],
        );
        let account = account(true);
        rig.service.usage(&account, false).await;
        rig.clock.advance(Duration::from_secs(5));
        rig.service.usage(&account, false).await;
        rig.clock.advance(Duration::from_secs(60));
        rig.service.usage(&account, false).await;
        rig.clock.advance(Duration::from_secs(4));
        assert_eq!(rig.api.tokens.lock().len(), 3);
        rig.service.usage(&account, false).await;
        assert_eq!(rig.api.tokens.lock().len(), 3);
        rig.clock.advance(Duration::from_secs(1));
        rig.service.usage(&account, false).await;
        assert_eq!(rig.api.tokens.lock().len(), 4);
    }

    #[tokio::test]
    async fn a_429_locks_the_endpoint_even_against_fresh_asks() {
        let rig = rig(vec![login("t", None)], vec![limited(None), reply(200, 3)]);
        let account = account(true);
        let usage = rig.service.usage(&account, false).await;
        assert_eq!(reason(&usage), Some("rate-limited"));
        rig.clock.advance(Duration::from_secs(299));
        let locked = rig.service.usage(&account, true).await;
        assert_eq!(reason(&locked), Some("rate-limited"));
        assert_eq!(rig.api.tokens.lock().len(), 1);
        rig.clock.advance(Duration::from_secs(1));
        assert!(rig.service.usage(&account, true).await.available());
    }

    #[tokio::test]
    async fn retry_after_may_lengthen_the_lockout_in_seconds_or_as_a_date() {
        let rig = rig(vec![login("t", None)], vec![]);
        let at = |text| rig.service.lockout(Some(text));
        assert_eq!(rig.service.lockout(None), RATE_LIMIT_TTL);
        assert_eq!(at("10"), RATE_LIMIT_TTL);
        assert_eq!(at("900"), Duration::from_mins(15));
        assert_eq!(at("soon"), RATE_LIMIT_TTL);
        assert_eq!(at(" "), RATE_LIMIT_TTL);
        assert_eq!(at("-5"), RATE_LIMIT_TTL);
        // the test clock stands at 1970-01-01T00:16:40Z
        assert_eq!(
            at("Thu, 01 Jan 1970 01:16:40 GMT"),
            Duration::from_secs(3600)
        );
        assert_eq!(at("Thu, 01 Jan 1970 00:20:00 GMT"), RATE_LIMIT_TTL);
    }

    #[tokio::test]
    async fn disabled_accounts_ask_nobody_and_show_their_email_only() {
        let clock = Arc::new(TestClock(AtomicI64::new(1)));
        let api = Arc::new(Api {
            replies: Mutex::new(VecDeque::new()),
            tokens: Mutex::new(Vec::new()),
        });
        let logins = Arc::new(Logins {
            reads: Mutex::new(vec![login("t", None)].into()),
            email: Some(String::from("max@example.com")),
        });
        let service = UsageService::new(logins, api.clone(), clock);
        let statuses = service.status(&[account(false)], false).await;
        assert_eq!(reason(&statuses[0].usage), Some("disabled"));
        assert!(statuses[0].logged_in);
        assert_eq!(statuses[0].email.as_deref(), Some("max@example.com"));
        assert_eq!(statuses[0].folder, "/a/max");
        assert!(api.tokens.lock().is_empty());
    }

    #[tokio::test]
    async fn logged_in_follows_the_email_or_any_reading_but_no_login() {
        let rig = rig(vec![LoginRead::Missing], vec![]);
        let statuses = rig.service.status(&[account(true)], false).await;
        assert!(!statuses[0].logged_in);
        let rig = self::rig(vec![login("t", None)], vec![reply(500, 0)]);
        let statuses = rig.service.status(&[account(true)], false).await;
        assert!(statuses[0].logged_in);
    }

    #[test]
    fn reasons_have_wire_names() {
        let names: Vec<&str> = [
            UsageReason::Disabled,
            UsageReason::NoLogin,
            UsageReason::TokenExpired,
            UsageReason::RequestFailed,
            UsageReason::RateLimited,
        ]
        .map(UsageReason::as_str)
        .to_vec();
        assert_eq!(
            names,
            [
                "disabled",
                "no-login",
                "token-expired",
                "request-failed",
                "rate-limited"
            ]
        );
    }
}
