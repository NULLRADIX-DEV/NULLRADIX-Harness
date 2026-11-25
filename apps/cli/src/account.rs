use std::io::{self, BufRead, IsTerminal};
use std::process::ExitCode;
use std::str::FromStr;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use nullradix_api::{
    AccountDto, AccountStatusDto, CreateAccount, SetRunToken, UpdateAccount, UsageReasonDto,
    UsageWindowDto,
};
use nullradix_client::Client;

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
        AccountCommand::Add { name, dir } => {
            let account = client.create_account(&CreateAccount {
                name,
                config_dir: dir,
            })?;
            println!("added {} ({})", account.slug, account.name);
        }
        AccountCommand::Usage { fresh, json } => usage(client, fresh, json)?,
        AccountCommand::Set(args) => set(client, args)?,
        AccountCommand::Enable { account } => switch(client, &account, true)?,
        AccountCommand::Disable { account } => switch(client, &account, false)?,
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
    format!(
        "{:<16} {:<20} {:<8} 5h {:<5} week {:<5} token {:<4} {}",
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
