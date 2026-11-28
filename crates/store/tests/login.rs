#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use nullradix_app::accounts::{AccountFolders, AccountService};
use nullradix_app::login::{
    LoginProcess, LoginRunner, LoginService, LoginSpec, LoginState, LoginStatus,
};
use nullradix_app::secrets::MemorySecretStore;
use nullradix_app::usage::{ClaudeLogins, LoginRead};
use nullradix_app::{AppError, ChangeSignal, ErrorKind, SystemClock, UuidV7Generator};
use nullradix_domain::{Account, AccountSlug, ConfigDir};
use nullradix_store::Store;
use tokio::sync::{Notify, mpsc, oneshot};

struct Folders;

#[async_trait]
impl AccountFolders for Folders {
    fn root(&self) -> String {
        String::from("/accounts")
    }

    async fn create(&self, slug: &AccountSlug) -> Result<ConfigDir, AppError> {
        Ok(ConfigDir::from_stored(format!("/accounts/{slug}")))
    }

    async fn check(&self, _dir: &ConfigDir) -> Result<(), AppError> {
        Ok(())
    }

    fn key(&self, dir: &ConfigDir) -> String {
        dir.as_str().to_owned()
    }

    fn slug_key(&self, slug: &AccountSlug) -> String {
        format!("/accounts/{slug}")
    }

    fn owns(&self, dir: &ConfigDir) -> bool {
        dir.as_str().starts_with("/accounts/")
    }

    async fn remove(&self, _dir: &ConfigDir) -> Result<(), AppError> {
        Ok(())
    }
}

/// The ends of a fake `claude auth login` that a test holds.
struct Control {
    output: mpsc::Sender<String>,
    exit: Option<oneshot::Sender<Option<i32>>>,
    input: mpsc::Receiver<String>,
    killed: Arc<AtomicBool>,
}

#[derive(Default)]
struct Runner {
    specs: Mutex<Vec<LoginSpec>>,
    controls: Mutex<Vec<Control>>,
    refuse: AtomicBool,
}

#[async_trait]
impl LoginRunner for Runner {
    async fn start(&self, spec: &LoginSpec) -> Result<LoginProcess, AppError> {
        if self.refuse.load(Ordering::SeqCst) {
            return Err(AppError::unavailable(
                "claude-not-found",
                "no claude on the path",
            ));
        }
        self.specs.lock().unwrap().push(spec.clone());
        let (output, lines) = mpsc::channel(16);
        let (exit, exited) = oneshot::channel();
        let (input_sender, input) = mpsc::channel(4);
        let killed = Arc::new(AtomicBool::new(false));
        let mark = killed.clone();
        self.controls.lock().unwrap().push(Control {
            output,
            exit: Some(exit),
            input,
            killed,
        });
        Ok(LoginProcess {
            output: lines,
            exit: exited,
            input: input_sender,
            kill: Box::new(move || mark.store(true, Ordering::SeqCst)),
        })
    }
}

/// The email of the login, and a gate that holds the reading of it until a test opens it.
struct Email(Mutex<Option<String>>, Mutex<Option<Arc<Notify>>>);

#[async_trait]
impl ClaudeLogins for Email {
    async fn read(&self, _dir: Option<&ConfigDir>) -> LoginRead {
        LoginRead::Missing
    }

    async fn email(&self, _dir: Option<&ConfigDir>) -> Option<String> {
        let gate = self.1.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        self.0.lock().unwrap().clone()
    }

    fn folder(&self, _dir: Option<&ConfigDir>) -> String {
        String::new()
    }
}

struct Fixture {
    logins: Arc<LoginService>,
    accounts: Arc<AccountService>,
    runner: Arc<Runner>,
    email: Arc<Email>,
}

async fn fixture_with(timeout: Option<Duration>) -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let accounts = Arc::new(AccountService::new(
        store.clone(),
        Arc::new(MemorySecretStore::new()),
        Arc::new(Folders),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
    ));
    let runner = Arc::new(Runner::default());
    let email = Arc::new(Email(Mutex::new(None), Mutex::new(None)));
    let mut logins = LoginService::new(
        runner.clone(),
        accounts.clone(),
        email.clone(),
        Arc::new(SystemClock),
    );
    if let Some(timeout) = timeout {
        logins = logins.with_timeout(timeout);
    }
    Fixture {
        logins: Arc::new(logins),
        accounts,
        runner,
        email,
    }
}

async fn fixture() -> Fixture {
    fixture_with(None).await
}

impl Fixture {
    async fn account(&self, name: &str) -> Account {
        self.accounts.create(name, None, None).await.unwrap()
    }

    fn control(&self, index: usize) -> Control {
        self.runner.controls.lock().unwrap().remove(index)
    }

    /// Waits until the login of `account` has this state; none when it never does.
    async fn settled(&self, account: &Account, state: LoginState) -> Option<LoginStatus> {
        for _ in 0..200 {
            if let Some(status) = self.logins.status(account.id())
                && status.state == state
            {
                return Some(status);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        None
    }
}

fn code(error: &AppError) -> (ErrorKind, &'static str) {
    (error.kind(), error.code())
}

#[tokio::test]
async fn a_login_starts_in_the_folder_of_the_account_and_collects_output_and_the_page() {
    let f = fixture().await;
    let max = f.account("Max").await;
    let started = f
        .logins
        .login(&max, Some(" me@example.com "))
        .await
        .unwrap();
    assert_eq!(started.state, LoginState::Running);
    assert_eq!(started.url, None);
    let specs = f.runner.specs.lock().unwrap().clone();
    assert_eq!(specs[0].config_dir, max.config_dir().cloned());
    assert_eq!(specs[0].email.as_deref(), Some("me@example.com"));

    let control = f.control(0);
    control
        .output
        .send("Opening browser\n".to_owned())
        .await
        .unwrap();
    control
        .output
        .send("Visit https://claude.ai/oauth/authorize?x=1 if it did not open\n".to_owned())
        .await
        .unwrap();
    for _ in 0..200 {
        if f.logins
            .status(max.id())
            .and_then(|status| status.url)
            .is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let status = f.logins.status(max.id()).unwrap();
    assert_eq!(
        status.url.as_deref(),
        Some("https://claude.ai/oauth/authorize?x=1")
    );
    assert!(status.output.starts_with("Opening browser\n"));
}

#[tokio::test]
async fn the_default_account_logs_in_without_a_folder() {
    let f = fixture().await;
    let default = f.accounts.find("default").await.unwrap();
    f.logins.login(&default, None).await.unwrap();
    let specs = f.runner.specs.lock().unwrap().clone();
    assert_eq!(specs[0].config_dir, None);
    assert_eq!(specs[0].email, None);
}

#[tokio::test]
async fn the_code_of_the_browser_reaches_the_process_and_only_while_it_runs() {
    let f = fixture().await;
    let max = f.account("Max").await;
    let empty = f.logins.code(&max, "abc").unwrap_err();
    assert_eq!(
        code(&empty),
        (ErrorKind::Conflict, "account-login-not-running")
    );

    f.logins.login(&max, None).await.unwrap();
    let mut control = f.control(0);
    f.logins.code(&max, "  the-code \n").unwrap();
    assert_eq!(control.input.recv().await.as_deref(), Some("the-code\n"));
    let blank = f.logins.code(&max, "  ").unwrap_err();
    assert_eq!(code(&blank), (ErrorKind::Invalid, "input-invalid"));

    control.exit.take().unwrap().send(Some(0)).unwrap();
    f.settled(&max, LoginState::Done).await.unwrap();
    let late = f.logins.code(&max, "again").unwrap_err();
    assert_eq!(
        code(&late),
        (ErrorKind::Conflict, "account-login-not-running")
    );
}

#[tokio::test]
async fn a_clean_exit_is_done_and_any_other_exit_is_a_failure() {
    let f = fixture().await;
    let ok = f.account("Ok").await;
    let bad = f.account("Bad").await;
    f.logins.login(&ok, None).await.unwrap();
    f.logins.login(&bad, None).await.unwrap();
    let mut first = f.control(0);
    let mut second = f.control(0);
    first.output.send("Logged in\n".to_owned()).await.unwrap();
    first.exit.take().unwrap().send(Some(0)).unwrap();
    second
        .output
        .send("error: denied\n".to_owned())
        .await
        .unwrap();
    second.exit.take().unwrap().send(Some(1)).unwrap();
    let done = f.settled(&ok, LoginState::Done).await.unwrap();
    let failed = f.settled(&bad, LoginState::Failed).await.unwrap();
    assert!(done.output.contains("Logged in"));
    assert!(failed.output.contains("error: denied"));
    assert!(second.killed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_signal_is_a_failure_too() {
    let f = fixture().await;
    let max = f.account("Max").await;
    f.logins.login(&max, None).await.unwrap();
    let mut control = f.control(0);
    control.exit.take().unwrap().send(None).unwrap();
    f.settled(&max, LoginState::Failed).await.unwrap();
}

#[tokio::test]
async fn a_new_login_ends_the_running_one() {
    let f = fixture().await;
    let max = f.account("Max").await;
    f.logins.login(&max, None).await.unwrap();
    let first = f.control(0);
    let again = f.logins.login(&max, None).await.unwrap();
    assert_eq!(again.state, LoginState::Running);
    assert!(first.killed.load(Ordering::SeqCst));
    assert_eq!(f.runner.controls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancelling_ends_the_process_and_may_forget_it() {
    let f = fixture().await;
    let max = f.account("Max").await;
    f.logins.login(&max, None).await.unwrap();
    let control = f.control(0);
    f.logins.cancel(max.id(), true);
    assert!(control.killed.load(Ordering::SeqCst));
    assert_eq!(
        f.logins.status(max.id()).unwrap().state,
        LoginState::Cancelled
    );
    f.logins.cancel(max.id(), false);
    assert_eq!(f.logins.status(max.id()), None);
    f.logins.cancel(max.id(), false);
}

#[tokio::test]
async fn closing_ends_every_running_login() {
    let f = fixture().await;
    let one = f.account("One").await;
    let two = f.account("Two").await;
    f.logins.login(&one, None).await.unwrap();
    f.logins.login(&two, None).await.unwrap();
    f.logins.close();
    for index in [0, 0] {
        assert!(f.control(index).killed.load(Ordering::SeqCst));
    }
    assert_eq!(
        f.logins.status(one.id()).unwrap().state,
        LoginState::Cancelled
    );
    assert_eq!(
        f.logins.status(two.id()).unwrap().state,
        LoginState::Cancelled
    );
}

#[tokio::test]
async fn a_login_that_takes_too_long_is_stopped() {
    let f = fixture_with(Some(Duration::from_millis(50))).await;
    let max = f.account("Max").await;
    f.logins.login(&max, None).await.unwrap();
    let control = f.control(0);
    let status = f.settled(&max, LoginState::Failed).await.unwrap();
    assert!(status.output.contains("took too long"));
    assert!(control.killed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn claude_that_cannot_start_leaves_a_failed_login_with_the_reason() {
    let f = fixture().await;
    let max = f.account("Max").await;
    f.runner.refuse.store(true, Ordering::SeqCst);
    let status = f.logins.login(&max, None).await.unwrap();
    assert_eq!(status.state, LoginState::Failed);
    assert!(status.output.contains("could not be started"));
    assert_eq!(f.logins.status(max.id()), Some(status));
    let late = f.logins.code(&max, "x").unwrap_err();
    assert_eq!(late.code(), "account-login-not-running");
}

#[tokio::test]
async fn emails_with_shell_characters_are_refused_before_anything_starts() {
    let f = fixture().await;
    let max = f.account("Max").await;
    let refused = f
        .logins
        .login(&max, Some("me@example.com & calc"))
        .await
        .unwrap_err();
    assert_eq!(code(&refused), (ErrorKind::Invalid, "account-invalid"));
    assert!(f.runner.specs.lock().unwrap().is_empty());
    let before = f.accounts.list().await.unwrap().len();
    let none = f
        .logins
        .create_and_login("New", Some("x y@example.com"))
        .await
        .unwrap_err();
    assert_eq!(none.code(), "account-invalid");
    assert_eq!(f.accounts.list().await.unwrap().len(), before);
}

#[tokio::test]
async fn a_new_account_without_a_name_is_named_after_the_email_once_logged_in() {
    let f = fixture().await;
    let (account, status) = f
        .logins
        .create_and_login("", Some("me@example.com"))
        .await
        .unwrap();
    assert_eq!(status.state, LoginState::Running);
    assert_eq!(account.name().as_str(), "me@example.com");
    assert_eq!(account.slug().as_str(), "me-example-com");
    *f.email.0.lock().unwrap() = Some(String::from("real@example.com"));
    let mut control = f.control(0);
    control.exit.take().unwrap().send(Some(0)).unwrap();
    f.settled(&account, LoginState::Done).await.unwrap();
    let renamed = f.accounts.get(account.id()).await.unwrap();
    assert_eq!(renamed.name().as_str(), "real@example.com");
    assert_eq!(renamed.slug().as_str(), "me-example-com");
}

#[tokio::test]
async fn a_named_account_keeps_its_name() {
    let f = fixture().await;
    let (account, _) = f
        .logins
        .create_and_login("Work", Some("me@example.com"))
        .await
        .unwrap();
    *f.email.0.lock().unwrap() = Some(String::from("real@example.com"));
    let mut control = f.control(0);
    control.exit.take().unwrap().send(Some(0)).unwrap();
    f.settled(&account, LoginState::Done).await.unwrap();
    assert_eq!(
        f.accounts.get(account.id()).await.unwrap().name().as_str(),
        "Work"
    );
}

#[tokio::test]
async fn without_a_name_or_email_the_account_is_called_new_account() {
    let f = fixture().await;
    let (account, _) = f.logins.create_and_login(" ", None).await.unwrap();
    assert_eq!(account.name().as_str(), "New account");
}

#[tokio::test]
async fn stopping_waits_for_the_process_to_exit() {
    let f = fixture().await;
    let max = f.account("Max").await;
    f.logins.login(&max, None).await.unwrap();
    let mut control = f.control(0);
    let logins = f.logins.clone();
    let id = max.id();
    let stopping = tokio::spawn(async move { logins.stop(id).await });
    for _ in 0..200 {
        if control.killed.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(control.killed.load(Ordering::SeqCst));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!stopping.is_finished(), "stopped before the process exited");
    control.exit.take().unwrap().send(None).unwrap();
    tokio::time::timeout(Duration::from_secs(2), stopping)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(f.logins.status(max.id()), None);
}

#[tokio::test]
async fn stopping_a_login_that_is_not_running_returns_at_once() {
    let f = fixture().await;
    let max = f.account("Max").await;
    f.logins.stop(max.id()).await;
    f.logins.login(&max, None).await.unwrap();
    f.logins.cancel(max.id(), true);
    tokio::time::timeout(Duration::from_millis(500), f.logins.stop(max.id()))
        .await
        .unwrap();
}

#[tokio::test]
async fn removing_an_account_whose_login_process_exited_does_not_wait() {
    let f = fixture().await;
    let gate = Arc::new(Notify::new());
    *f.email.0.lock().unwrap() = Some(String::from("me@example.com"));
    *f.email.1.lock().unwrap() = Some(gate.clone());
    let (account, _) = f.logins.create_and_login(" ", None).await.unwrap();
    let mut control = f.control(0);
    drop(control.output);
    control.exit.take().unwrap().send(Some(0)).unwrap();
    // the watcher now reads the email for the new name, which needs the lock of the removal
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    let removal = tokio::time::timeout(
        Duration::from_millis(1500),
        f.accounts
            .remove_with(account.id(), account.version(), false, &*f.logins),
    );
    let release = async {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        gate.notify_one();
    };
    let (removed, ()) = tokio::join!(removal, release);
    removed
        .expect("the removal waited for the exited login")
        .unwrap();
    assert!(f.accounts.get(account.id()).await.is_err());
}
