use std::io::{self, BufRead, IsTerminal};
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use nullradix_api::{
    AccountDto, AccountStatusDto, CreateAccount, CreateAccountLogin, LoginCode, LoginDto,
    LoginStateDto, SetRunToken, StartLogin, UpdateAccount, UsageReasonDto, UsageWindowDto,
};
use nullradix_client::Client;

/// How often a running login is looked at.
const LOGIN_POLL: Duration = Duration::from_secs(1);

#[derive(Subcommand)]
pub(crate) enum AccountCommand {
    /// List the Claude accounts runs are spread over
    List {
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Show the login and the usage limits of every account
    Usage {
        /// Ask Anthropic again instead of using a reading of the last minute
        #[arg(long)]
        fresh: bool,
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Add an account with its own Claude folder, or take over an existing one
    Add {
        /// Display name; the account id is made from it
        name: String,
        /// Existing Claude folder to take over, such as ~/.mcacc/work
        #[arg(long)]
        dir: Option<String>,
        /// Log the account in with Claude right away
        #[arg(long, conflicts_with = "dir")]
        login: bool,
        /// Email the login page of the browser starts with
        #[arg(long, requires = "login")]
        email: Option<String>,
    },
    /// Log an account in with `claude auth login`, which opens the browser
    Login {
        /// Account id, as `nullradix account list` shows it
        account: String,
        /// Email the login page of the browser starts with
        #[arg(long)]
        email: Option<String>,
        /// Start the login and return, without following it
        #[arg(long)]
        no_wait: bool,
    },
    /// Change name, fallback switch or limits of an account
    Set(SetArgs),
    /// Let runs use the account
    Enable {
        /// Account id, as `nullradix account list` shows it
        account: String,
    },
    /// Keep runs away from the account
    Disable {
        /// Account id, as `nullradix account list` shows it
        account: String,
    },
    /// Let the dispatcher use a paused account again before its pause ends
    Resume {
        /// Account id, as `nullradix account list` shows it
        account: String,
    },
    /// Remove an account and its stored token
    Remove {
        /// Account id, as `nullradix account list` shows it
        account: String,
        /// Also delete the account's own Claude folder, with its login
        #[arg(long)]
        delete_dir: bool,
    },
    /// Store or forget the long-lived token of `claude setup-token`
    #[command(subcommand)]
    Token(TokenCommand),
}

#[derive(Subcommand)]
pub(crate) enum TokenCommand {
    /// Read the token from standard input and keep it in the secret store
    Set {
        /// Account id, as `nullradix account list` shows it
        account: String,
    },
    /// Forget the token; runs use the login in the account's folder again
    Clear {
        /// Account id, as `nullradix account list` shows it
        account: String,
    },
}

#[derive(Args)]
pub(crate) struct SetArgs {
    /// Account id, as `nullradix account list` shows it
    account: String,
    /// New display name
    #[arg(long)]
    name: Option<String>,
    /// Use the account only when no other allowed account is free, or not (true or false)
    #[arg(long, value_name = "BOOL")]
    fallback: Option<bool>,
    /// Usage ceiling in percent for the five hour window, or `none` for the default
    #[arg(long, value_name = "PERCENT|none")]
    limit_five_hour: Option<LimitArg>,
    /// Usage ceiling in percent for the week, or `none` for the default
    #[arg(long, value_name = "PERCENT|none")]
    limit_week: Option<LimitArg>,
}

/// A limit given on the command line; the daemon checks the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LimitArg {
    Percent(i64),
    Default,
}

impl FromStr for LimitArg {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.eq_ignore_ascii_case("none") {
            return Ok(Self::Default);
        }
        text.trim_end_matches('%')
            .parse()
            .map(Self::Percent)
            .map_err(|_| format!("`{text}` is neither a whole number nor `none`"))
    }
}

impl LimitArg {
    const fn value(self) -> Option<i64> {
        match self {
            Self::Percent(percent) => Some(percent),
            Self::Default => None,
        }
    }
}

pub(crate) fn run(command: AccountCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        AccountCommand::List { json } => list(client, json)?,
        AccountCommand::Add {
            name,
            dir,
            login,
            email,
        } => add(client, name, dir, login, email)?,
        AccountCommand::Login {
            account,
            email,
            no_wait,
        } => {
            let found = find(client, &account)?;
            let started = client.start_login(found.id, &StartLogin { email })?;
            println!("login started for {account}");
            if !no_wait {
                follow_login(client, &found, started)?;
            }
        }
        AccountCommand::Usage { fresh, json } => usage(client, fresh, json)?,
        AccountCommand::Set(args) => set(client, args)?,
        AccountCommand::Enable { account } => switch(client, &account, true)?,
        AccountCommand::Disable { account } => switch(client, &account, false)?,
        AccountCommand::Resume { account } => {
            let found = find(client, &account)?;
            let resumed = client.resume_account(found.id)?;
            println!("{}", describe(&resumed));
        }
        AccountCommand::Remove {
            account,
            delete_dir,
        } => {
            let found = find(client, &account)?;
            client
                .delete_account(found.id, found.version, delete_dir)
                .with_context(|| format!("cannot remove {account}"))?;
            println!("removed {account}");
        }
        AccountCommand::Token(TokenCommand::Set { account }) => {
            let found = find(client, &account)?;
            let token = read_token(&mut io::stdin().lock(), io::stdin().is_terminal())?;
            client
                .set_run_token(
                    found.id,
                    &SetRunToken {
                        token,
                        version: found.version,
                    },
                )
                .with_context(|| format!("cannot store the token of {account}"))?;
            println!("token stored for {account}");
        }
        AccountCommand::Token(TokenCommand::Clear { account }) => {
            let found = find(client, &account)?;
            client.clear_run_token(found.id, found.version)?;
            println!("token cleared for {account}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn list(client: &Client, json: bool) -> anyhow::Result<()> {
    let accounts = client.list_accounts()?.items;
    if json {
        println!("{}", serde_json::to_string_pretty(&accounts)?);
        return Ok(());
    }
    for account in &accounts {
        println!("{}", describe(account));
    }
    Ok(())
}

fn set(client: &Client, args: SetArgs) -> anyhow::Result<()> {
    let account = find(client, &args.account)?;
    change(
        client,
        &account,
        &UpdateAccount {
            version: account.version,
            name: args.name,
            fallback: args.fallback,
            limit_five_hour: args.limit_five_hour.map(LimitArg::value),
            limit_week: args.limit_week.map(LimitArg::value),
            ..UpdateAccount::default()
        },
    )
}

fn switch(client: &Client, slug: &str, enabled: bool) -> anyhow::Result<()> {
    let account = find(client, slug)?;
    change(
        client,
        &account,
        &UpdateAccount {
            version: account.version,
            enabled: Some(enabled),
            ..UpdateAccount::default()
        },
    )
}

fn change(client: &Client, account: &AccountDto, update: &UpdateAccount) -> anyhow::Result<()> {
    let changed = client.update_account(account.id, update)?;
    println!("updated {}", describe(&changed));
    Ok(())
}

fn add(
    client: &Client,
    name: String,
    dir: Option<String>,
    login: bool,
    email: Option<String>,
) -> anyhow::Result<()> {
    if !login {
        let account = client.create_account(&CreateAccount {
            name,
            config_dir: dir,
        })?;
        println!("added {} ({})", account.slug, account.name);
        return Ok(());
    }
    let started = client.create_account_login(&CreateAccountLogin {
        name: Some(name),
        email,
    })?;
    println!("added {} ({})", started.account.slug, started.account.name);
    follow_login(client, &started.account, started.login)
}

/// Waits for a login, shows its page and forwards the code that a person pastes.
fn follow_login(client: &Client, account: &AccountDto, first: LoginDto) -> anyhow::Result<()> {
    let typed = std::io::stdin().is_terminal().then(|| {
        let (sender, receiver) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in io::stdin().lock().lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        receiver
    });
    if typed.is_some() {
        eprintln!("If Claude asks for a code, paste it here and press Enter.");
    }
    let mut shown_url = false;
    let mut login = first;
    loop {
        if let Some(url) = login.url.as_deref().filter(|_| !shown_url) {
            eprintln!("If no browser opened, visit {url}");
            shown_url = true;
        }
        match login.state {
            LoginStateDto::Running => {}
            LoginStateDto::Done => {
                println!("{} is logged in", account.slug);
                return Ok(());
            }
            LoginStateDto::Failed => bail!("the login failed:\n{}", failure(&login.output)),
            LoginStateDto::Cancelled => bail!("the login was cancelled"),
        }
        match typed.as_ref().map(|lines| lines.recv_timeout(LOGIN_POLL)) {
            Some(Ok(line)) if !line.trim().is_empty() => {
                client.send_login_code(account.id, &LoginCode { code: line })?;
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => std::thread::sleep(LOGIN_POLL),
        }
        login = client.get_login(account.id)?;
    }
}

/// The last lines of what Claude Code printed, without the login page and the prompts.
fn failure(output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.contains("https://"))
        .collect();
    let start = lines.len().saturating_sub(6);
    lines[start..].join("\n")
}

fn usage(client: &Client, fresh: bool, json: bool) -> anyhow::Result<()> {
    let items = client.account_usage(fresh)?.items;
    if json {
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    for status in &items {
        println!("{}", describe_usage(status));
    }
    Ok(())
}

fn window(name: &str, window: Option<UsageWindowDto>) -> String {
    match window {
        Some(window) => match window.resets_at {
            Some(at) => format!(
                "{name} {}% (resets {})",
                window.percent,
                nullradix_domain::Timestamp::from_millis(at)
            ),
            None => format!("{name} {}%", window.percent),
        },
        None => format!("{name} -"),
    }
}

const fn reason_text(reason: UsageReasonDto) -> &'static str {
    match reason {
        UsageReasonDto::Disabled => "the account is disabled",
        UsageReasonDto::NoLogin => "no Claude login found in the account's folder",
        UsageReasonDto::TokenExpired => "the login expired, log in again with Claude Code",
        UsageReasonDto::RequestFailed => "Anthropic did not answer",
        UsageReasonDto::RateLimited => "Anthropic asked to wait before the next request",
    }
}

/// One line per account: id, login, plan and the two limit windows, or why there are none.
fn describe_usage(status: &AccountStatusDto) -> String {
    let login = match (&status.email, status.logged_in) {
        (Some(email), _) => email.clone(),
        (None, true) => String::from("logged in"),
        (None, false) => String::from("not logged in"),
    };
    let numbers = match status.usage.reason {
        Some(reason) => format!("no limits: {}", reason_text(reason)),
        None => format!(
            "{}  {}",
            window("5h", status.usage.five_hour),
            window("week", status.usage.seven_day)
        ),
    };
    let plan = status.usage.plan.as_deref().unwrap_or("-");
    format!(
        "{:<16} {:<24} plan {:<6} {numbers}",
        status.slug, login, plan
    )
}

/// One line per account: id, name, switches, limits and whether a run token is stored.
fn describe(account: &AccountDto) -> String {
    let limit =
        |value: Option<i64>| value.map_or_else(|| String::from("-"), |pct| format!("{pct}%"));
    let state = match (account.enabled, account.fallback) {
        (false, _) => "disabled",
        (true, true) => "fallback",
        (true, false) => "enabled",
    };
    let token = if account.has_run_token { "set" } else { "none" };
    let folder = account
        .config_dir
        .as_deref()
        .map_or_else(|| String::from("the server's own login"), str::to_owned);
    let pause = account.paused_until.map_or_else(String::new, |until| {
        format!(
            "  paused until {} ({})",
            nullradix_domain::Timestamp::from_millis(until),
            account.pause_reason.as_deref().unwrap_or("-")
        )
    });
    format!(
        "{:<16} {:<20} {:<8} 5h {:<5} week {:<5} token {:<4} {}{pause}",
        account.slug,
        account.name,
        state,
        limit(account.limit_five_hour),
        limit(account.limit_week),
        token,
        folder
    )
}

/// The account with this id, as the list shows it.
fn find(client: &Client, slug: &str) -> anyhow::Result<AccountDto> {
    let accounts = client.list_accounts()?.items;
    match accounts.into_iter().find(|account| account.slug == slug) {
        Some(account) => Ok(account),
        None => bail!("no account has the id {slug}, `nullradix account list` shows them"),
    }
}

/// The token on one line of standard input; people get a prompt, pipes do not.
fn read_token(input: &mut impl BufRead, terminal: bool) -> anyhow::Result<String> {
    if terminal {
        eprintln!("Paste the token of `claude setup-token` and press Enter:");
    }
    let mut line = String::new();
    input
        .read_line(&mut line)
        .context("cannot read the token from standard input")?;
    let token = line.trim();
    if token.is_empty() {
        bail!("no token on standard input, pipe it in or paste it when asked");
    }
    Ok(token.to_owned())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn account() -> AccountDto {
        AccountDto {
            id: Uuid::from_u128(1),
            slug: String::from("max"),
            name: String::from("Max"),
            config_dir: Some(String::from("/data/accounts/max")),
            enabled: true,
            fallback: false,
            limit_five_hour: Some(50),
            limit_week: None,
            has_run_token: true,
            paused_until: None,
            pause_reason: None,
            created_at: 1,
            updated_at: 1,
            version: 1,
        }
    }

    fn status(reason: Option<UsageReasonDto>) -> AccountStatusDto {
        AccountStatusDto {
            account_id: Uuid::from_u128(1),
            slug: String::from("max"),
            logged_in: true,
            email: Some(String::from("max@example.com")),
            folder: String::from("/a"),
            login: None,
            usage: nullradix_api::UsageDto {
                available: reason.is_none(),
                reason,
                plan: Some(String::from("max")),
                five_hour: Some(UsageWindowDto {
                    percent: 42,
                    resets_at: Some(1_700_000_000_123),
                }),
                seven_day: Some(UsageWindowDto {
                    percent: 7,
                    resets_at: None,
                }),
                fetched_at: Some(1),
            },
        }
    }

    #[test]
    fn describes_usage_with_windows_or_the_reason_for_none() {
        let line = describe_usage(&status(None));
        assert!(line.starts_with("max "));
        assert!(line.contains("max@example.com"));
        assert!(line.contains("plan max"));
        assert!(line.contains("5h 42% (resets 2023-11-14T22:13:20.123Z)"));
        assert!(line.ends_with("week 7%"));
        let none = describe_usage(&status(Some(UsageReasonDto::NoLogin)));
        assert!(none.contains("no limits: no Claude login found"));
        assert!(!none.contains("5h"));
        let mut quiet = status(None);
        quiet.email = None;
        quiet.logged_in = false;
        quiet.usage.five_hour = None;
        quiet.usage.plan = None;
        let line = describe_usage(&quiet);
        assert!(line.contains("not logged in"));
        assert!(line.contains("plan -"));
        assert!(line.contains("5h -"));
    }
    #[test]
    fn limits_are_numbers_or_none() {
        assert_eq!("80".parse(), Ok(LimitArg::Percent(80)));
        assert_eq!("80%".parse(), Ok(LimitArg::Percent(80)));
        assert_eq!("None".parse(), Ok(LimitArg::Default));
        assert!("many".parse::<LimitArg>().is_err());
        assert_eq!(LimitArg::Percent(80).value(), Some(80));
        assert_eq!(LimitArg::Default.value(), None);
    }

    #[test]
    fn describes_an_account_without_any_secret() {
        let line = describe(&account());
        assert!(line.starts_with("max "));
        assert!(line.contains("enabled"));
        assert!(line.contains("5h 50%"));
        assert!(line.contains("week -"));
        assert!(line.contains("token set"));
        assert!(line.ends_with("/data/accounts/max"));
        let mut off = account();
        off.enabled = false;
        off.config_dir = None;
        off.has_run_token = false;
        let line = describe(&off);
        assert!(line.contains("disabled"));
        assert!(line.contains("token none"));
        assert!(line.contains("the server's own login"));
        off.enabled = true;
        off.fallback = true;
        assert!(describe(&off).contains("fallback"));
        off.paused_until = Some(1_800_000_000_000);
        off.pause_reason = Some(String::from("usage-limit"));
        assert!(describe(&off).ends_with("  paused until 2027-01-15T08:00:00.000Z (usage-limit)"));
    }

    #[test]
    fn reads_the_token_from_the_first_line() {
        let mut input = "  sk-ant-oat01-fake \nsecond line\n".as_bytes();
        assert_eq!(read_token(&mut input, false).unwrap(), "sk-ant-oat01-fake");
    }

    #[test]
    fn an_empty_input_is_no_token() {
        for text in ["", "\n", "   \n"] {
            let error = read_token(&mut text.as_bytes(), false).unwrap_err();
            assert!(error.to_string().contains("no token"));
        }
    }
}
