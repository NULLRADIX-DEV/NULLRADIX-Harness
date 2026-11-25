//! Logs an account in through `claude auth login`, which opens the browser itself.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_domain::{Account, ConfigDir, Id, Timestamp};
use parking_lot::Mutex;
use strum::IntoStaticStr;
use tokio::sync::{mpsc, oneshot};

use crate::accounts::{AccountPatch, AccountService};
use crate::usage::ClaudeLogins;
use crate::{AppError, Clock};

/// A login that takes longer is given up.
pub const LOGIN_TIMEOUT: Duration = Duration::from_mins(10);
/// How much of the output stays; the end holds the reason of a failure.
pub const OUTPUT_MAX_CHARS: usize = 4_000;
/// How long the last lines may still arrive once the process has ended.
const OUTPUT_GRACE: Duration = Duration::from_millis(500);
pub const EMAIL_MAX: usize = 254;

/// What to start: the folder Claude Code keeps the login in, none for the server's own login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSpec {
    pub config_dir: Option<ConfigDir>,
    /// Filled into the login page of the browser.
    pub email: Option<String>,
}

/// A running `claude auth login`.
pub struct LoginProcess {
    /// Standard output and error, line by line.
    pub output: mpsc::Receiver<String>,
    /// Ends with the process; the value is its exit code, none when a signal ended it.
    pub exit: oneshot::Receiver<Option<i32>>,
    /// What the person pasted from the browser.
    pub input: mpsc::Sender<String>,
    pub kill: Box<dyn Fn() + Send + Sync>,
}

#[async_trait]
pub trait LoginRunner: Send + Sync {
    /// Fails when Claude Code cannot be started.
    async fn start(&self, spec: &LoginSpec) -> Result<LoginProcess, AppError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
pub enum LoginState {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl LoginState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginStatus {
    pub state: LoginState,
    pub started_at: Timestamp,
    /// The login page, for when no browser opened.
    pub url: Option<String>,
    /// The end of what Claude Code printed.
    pub output: String,
}

#[must_use]
pub fn not_running(account: &nullradix_domain::AccountSlug) -> AppError {
    AppError::conflict(
        "account-login-not-running",
        format!("no login is running for account {account}"),
    )
}

#[must_use]
pub fn email_invalid(email: &str) -> AppError {
    AppError::invalid(
        "account-invalid",
        format!("{email} does not look like an email address"),
    )
}

#[must_use]
pub fn claude_not_found(program: &str) -> AppError {
    AppError::unavailable(
        "claude-not-found",
        format!("Claude Code was not found as {program}, install it or put it on the path"),
    )
}
#[must_use]
pub fn code_empty() -> AppError {
    AppError::invalid("input-invalid", "the code is empty")
}

#[must_use]
pub fn login_not_found(account: &nullradix_domain::AccountSlug) -> AppError {
    AppError::not_found(
        "login-not-found",
        format!("account {account} has had no login yet"),
    )
}

/// An email that is safe on any command line: no spaces, quotes or shell characters.
fn checked_email(email: &str) -> Result<String, AppError> {
    let email = email.trim();
    let allowed = |letter: char| letter.is_ascii_alphanumeric() || "._+-@".contains(letter);
    let valid = !email.is_empty()
        && email.len() <= EMAIL_MAX
        && email.chars().all(allowed)
        && email.matches('@').count() == 1
        && !email.starts_with(['@', '-'])
        && !email.ends_with('@');
    if valid {
        Ok(email.to_owned())
    } else {
        Err(email_invalid(email))
    }
}

/// The last [`OUTPUT_MAX_CHARS`] characters.
fn tail(text: &str) -> String {
    let count = text.chars().count();
    text.chars()
        .skip(count.saturating_sub(OUTPUT_MAX_CHARS))
        .collect()
}

/// The first `https` address in the output, without trailing punctuation.
fn first_url(output: &str) -> Option<String> {
    let start = output.find("https://")?;
    let url: String = output[start..]
        .chars()
        .take_while(|letter| !letter.is_whitespace())
        .collect();
    let url = url.trim_end_matches([')', ']', '.', ',', '\'', '"']);
    (url.len() > "https://".len()).then(|| url.to_owned())
}

struct Login {
    status: Arc<Mutex<LoginStatus>>,
    input: Option<mpsc::Sender<String>>,
    kill: Option<Arc<dyn Fn() + Send + Sync>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Login {
    fn running(&self) -> bool {
        self.status.lock().state == LoginState::Running
    }
}

pub struct LoginService {
    runner: Arc<dyn LoginRunner>,
    accounts: Arc<AccountService>,
    logins: Arc<dyn ClaudeLogins>,
    clock: Arc<dyn Clock>,
    timeout: Duration,
    running: Mutex<HashMap<Id<Account>, Login>>,
}

impl std::fmt::Debug for LoginService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginService").finish_non_exhaustive()
    }
}

impl LoginService {
    #[must_use]
    pub fn new(
        runner: Arc<dyn LoginRunner>,
        accounts: Arc<AccountService>,
        logins: Arc<dyn ClaudeLogins>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            runner,
            accounts,
            logins,
            clock,
            timeout: LOGIN_TIMEOUT,
            running: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Adds an account and logs it in; without a name it is named after the email once logged in.
    pub async fn create_and_login(
        self: &Arc<Self>,
        name: &str,
        email: Option<&str>,
    ) -> Result<(Account, LoginStatus), AppError> {
        let email = email
            .map(str::trim)
            .filter(|email| !email.is_empty())
            .map(checked_email)
            .transpose()?;
        let named = !name.trim().is_empty();
        let shown = if named {
            name
        } else {
            email.as_deref().unwrap_or("New account")
        };
        let account = self.accounts.create(shown, None).await?;
        let status = self.start(&account, email, !named).await?;
        Ok((account, status))
    }

    /// Starts the login of an account; one that is still running is ended first.
    pub async fn login(
        self: &Arc<Self>,
        account: &Account,
        email: Option<&str>,
    ) -> Result<LoginStatus, AppError> {
        let email = email
            .map(str::trim)
            .filter(|email| !email.is_empty())
            .map(checked_email)
            .transpose()?;
        self.start(account, email, false).await
    }

    async fn start(
        self: &Arc<Self>,
        account: &Account,
        email: Option<String>,
        rename_to_email: bool,
    ) -> Result<LoginStatus, AppError> {
        self.cancel(account.id(), false);
        let spec = LoginSpec {
            config_dir: account.config_dir().cloned(),
            email,
        };
        let status = Arc::new(Mutex::new(LoginStatus {
            state: LoginState::Running,
            started_at: self.clock.now(),
            url: None,
            output: String::new(),
        }));
        let process = match self.runner.start(&spec).await {
            Ok(process) => process,
            Err(error) => {
                {
                    let mut seen = status.lock();
                    seen.state = LoginState::Failed;
                    seen.output = format!("Claude Code could not be started: {}", error.message());
                }
                let shown = status.lock().clone();
                self.running.lock().insert(
                    account.id(),
                    Login {
                        status,
                        input: None,
                        kill: None,
                        task: None,
                    },
                );
                return Ok(shown);
            }
        };
        let kill: Arc<dyn Fn() + Send + Sync> = Arc::from(process.kill);
        let input = process.input.clone();
        let watcher = Watch {
            service: Arc::clone(self),
            status: Arc::clone(&status),
            account: account.clone(),
            rename_to_email,
            kill: Arc::clone(&kill),
        };
        let task = tokio::spawn(watcher.run(process.output, process.exit));
        let shown = status.lock().clone();
        self.running.lock().insert(
            account.id(),
            Login {
                status,
                input: Some(input),
                kill: Some(kill),
                task: Some(task),
            },
        );
        Ok(shown)
    }

    /// How the last login of the account stands, if there was one.
    #[must_use]
    pub fn status(&self, account: Id<Account>) -> Option<LoginStatus> {
        self.running
            .lock()
            .get(&account)
            .map(|login| login.status.lock().clone())
    }

    /// Hands the code of the browser to the running login.
    pub fn code(&self, account: &Account, code: &str) -> Result<(), AppError> {
        let sender = {
            let running = self.running.lock();
            running
                .get(&account.id())
                .filter(|login| login.running())
                .and_then(|login| login.input.clone())
        };
        let Some(sender) = sender else {
            return Err(not_running(account.slug()));
        };
        let code = code.trim();
        if code.is_empty() {
            return Err(code_empty());
        }
        sender
            .try_send(format!("{code}\n"))
            .map_err(|_| not_running(account.slug()))
    }

    /// Ends a running login; `keep` false also forgets how it went.
    pub fn cancel(&self, account: Id<Account>, keep: bool) {
        let mut running = self.running.lock();
        if let Some(login) = running.get_mut(&account) {
            if login.running() {
                login.status.lock().state = LoginState::Cancelled;
                if let Some(kill) = &login.kill {
                    kill();
                }
            }
            if let Some(task) = login.task.take() {
                task.abort();
            }
        }
        if !keep {
            running.remove(&account);
        }
    }

    /// Ends every login, as the daemon stops.
    pub fn close(&self) {
        let ids: Vec<Id<Account>> = self.running.lock().keys().copied().collect();
        for id in ids {
            self.cancel(id, true);
        }
    }

    /// Names the account after the email its login belongs to.
    async fn rename_to_email(&self, account: &Account) {
        let Some(email) = self.logins.email(account.config_dir()).await else {
            return;
        };
        let patch = AccountPatch {
            name: Some(email),
            ..AccountPatch::default()
        };
        let Ok(current) = self.accounts.get(account.id()).await else {
            return;
        };
        if let Err(error) = self
            .accounts
            .update(current.id(), patch, current.version())
            .await
        {
            tracing::debug!(code = error.code(), "account not renamed after its login");
        }
    }
}

/// Follows one running login: collects its output, watches the clock and settles the outcome.
struct Watch {
    service: Arc<LoginService>,
    status: Arc<Mutex<LoginStatus>>,
    account: Account,
    rename_to_email: bool,
    kill: Arc<dyn Fn() + Send + Sync>,
}

impl Watch {
    /// Adds what Claude Code printed, and keeps the first login page it names.
    fn push(&self, chunk: &str) {
        let mut seen = self.status.lock();
        seen.output = tail(&format!("{}{chunk}", seen.output));
        if seen.url.is_none() {
            seen.url = first_url(&seen.output);
        }
    }

    async fn run(
        self,
        mut output: mpsc::Receiver<String>,
        mut exit: oneshot::Receiver<Option<i32>>,
    ) {
        let collect = async {
            while let Some(chunk) = output.recv().await {
                self.push(&chunk);
            }
        };
        let code = tokio::select! {
            () = collect => (&mut exit).await.ok().flatten(),
            result = &mut exit => result.ok().flatten(),
            () = tokio::time::sleep(self.service.timeout) => {
                {
                    let mut seen = self.status.lock();
                    seen.output = tail(&format!("{}\nThe login took too long and was stopped.", seen.output));
                    seen.state = LoginState::Failed;
                }
                (self.kill)();
                return;
            }
        };
        // the readers may still hold the last lines when the process has ended
        let rest = async {
            while let Some(chunk) = output.recv().await {
                self.push(&chunk);
            }
        };
        let _ = tokio::time::timeout(OUTPUT_GRACE, rest).await;
        if self.status.lock().state != LoginState::Running {
            return;
        }
        if code != Some(0) {
            self.status.lock().state = LoginState::Failed;
            (self.kill)();
            return;
        }
        if self.rename_to_email {
            self.service.rename_to_email(&self.account).await;
        }
        let mut seen = self.status.lock();
        if seen.state == LoginState::Running {
            seen.state = LoginState::Done;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_checked_for_the_command_line() {
        for good in ["me@example.com", "a.b+c@d-e.org", "x_y@host"] {
            assert_eq!(checked_email(&format!(" {good} ")).unwrap(), good);
        }
        for bad in [
            "",
            "me",
            "@example.com",
            "me@",
            "a@b@c",
            "me@example.com & calc",
            "me@example.com\"",
            "-me@example.com",
            "me@exa mple.com",
            "me@example.com|x",
            "100%@example.com",
            "me@exa%41mple.com",
            &format!("{}@x.com", "a".repeat(250)),
        ] {
            let error = checked_email(bad).unwrap_err();
            assert_eq!(error.code(), "account-invalid", "{bad}");
        }
    }

    #[test]
    fn the_output_keeps_its_end() {
        assert_eq!(tail("short"), "short");
        let long = format!("{}end", "x".repeat(OUTPUT_MAX_CHARS));
        let kept = tail(&long);
        assert_eq!(kept.chars().count(), OUTPUT_MAX_CHARS);
        assert!(kept.ends_with("xend"));
    }

    #[test]
    fn finds_the_login_page_in_the_output() {
        assert_eq!(first_url("no link here"), None);
        assert_eq!(
            first_url(
                "Opening browser\nVisit https://claude.ai/oauth/authorize?code=true&x=1 to continue"
            ),
            Some(String::from(
                "https://claude.ai/oauth/authorize?code=true&x=1"
            ))
        );
        assert_eq!(
            first_url("(see https://claude.ai/login)."),
            Some(String::from("https://claude.ai/login"))
        );
        assert_eq!(first_url("https://"), None);
    }

    #[test]
    fn states_have_wire_names() {
        let names: Vec<&str> = [
            LoginState::Running,
            LoginState::Done,
            LoginState::Failed,
            LoginState::Cancelled,
        ]
        .map(LoginState::as_str)
        .to_vec();
        assert_eq!(names, ["running", "done", "failed", "cancelled"]);
    }
}
