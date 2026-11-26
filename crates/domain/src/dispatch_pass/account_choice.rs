use std::collections::HashMap;
use std::time::Duration;

use crate::{Account, AccountSlug, AccountStrategy, DispatcherConfig, Id, Timestamp};

use super::profile_choice::{ProfileChoice, ProfileReason};

/// Label that pins a ticket to one account, such as `account:max`.
pub const ACCOUNT_LABEL_PREFIX: &str = "account:";

const MINUTE: Duration = Duration::from_mins(1);

/// Expected use of one run in percentage points per limit window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccountReserve {
    pub five_hour: i64,
    pub week: i64,
}

impl AccountReserve {
    #[must_use]
    pub const fn plus(self, other: Self) -> Self {
        Self {
            five_hour: self.five_hour + other.five_hour,
            week: self.week + other.week,
        }
    }
}

/// How full the limit windows of an account are; none means unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub five_hour: Option<i64>,
    pub seven_day: Option<i64>,
    pub five_hour_resets_at: Option<Timestamp>,
    pub seven_day_resets_at: Option<Timestamp>,
}

/// What is known of an account when a run starts; unknown never excludes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccountState {
    pub logged_in: Option<bool>,
    pub usage: UsageSnapshot,
    pub paused_until: Option<Timestamp>,
}

/// The accounts a run may use and where that list came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllowedAccounts {
    /// Account slugs in order; empty means every account.
    pub slugs: Vec<String>,
    pub from: Option<String>,
}

/// The slug of the first `account:` label, as written.
#[must_use]
pub fn account_label(labels: &[String]) -> Option<&str> {
    labels
        .iter()
        .find_map(|label| label.strip_prefix(ACCOUNT_LABEL_PREFIX))
        .map(str::trim)
        .filter(|slug| !slug.is_empty())
}

/// The accounts of a run: an `account:` label, else the matched rule, else the profile.
#[must_use]
pub fn allowed_accounts(
    config: &DispatcherConfig,
    choice: &ProfileChoice,
    labels: &[String],
) -> AllowedAccounts {
    if let Some(slug) = account_label(labels).filter(|slug| AccountSlug::parse(slug).is_ok()) {
        return AllowedAccounts {
            slugs: vec![slug.to_owned()],
            from: Some(format!("label {ACCOUNT_LABEL_PREFIX}{slug}")),
        };
    }
    // an escalated profile brings its own accounts
    let rule = choice
        .rule
        .filter(|_| choice.reason == ProfileReason::Rule && choice.escalated_from.is_none())
        .and_then(|place| {
            config
                .rules
                .get(place.wrapping_sub(1))
                .map(|rule| (place, rule))
        });
    if let Some((place, rule)) = rule
        && !rule.accounts.is_empty()
    {
        return AllowedAccounts {
            slugs: rule.accounts.clone(),
            from: Some(format!("rule {place}")),
        };
    }
    let slugs = config
        .profile(&choice.profile)
        .map_or_else(Vec::new, |profile| profile.accounts.clone());
    AllowedAccounts {
        from: (!slugs.is_empty()).then(|| format!("profile {}", choice.profile)),
        slugs,
    }
}

/// Everything the choice looks at.
#[derive(Debug, Clone, Copy)]
pub struct AccountChoiceInput<'a> {
    pub allowed: &'a AllowedAccounts,
    /// All accounts in their order.
    pub accounts: &'a [Account],
    pub states: &'a HashMap<Id<Account>, AccountState>,
    pub strategy: AccountStrategy,
    /// Default ceiling in percent for accounts without their own.
    pub limit_percent: i64,
    pub now: Timestamp,
    /// Expected use of this run.
    pub reserve: AccountReserve,
    /// Expected use still to come from runs under way, per account.
    pub pending: &'a HashMap<Id<Account>, AccountReserve>,
    /// A five hour window that resets within this many minutes ignores the reserve.
    pub reset_grace_minutes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountChoice {
    Picked {
        account: Id<Account>,
        slug: String,
        why: String,
        /// How full the account was at this moment.
        snapshot: UsageSnapshot,
    },
    /// No account is free: why, and when one may be.
    Waiting {
        reason: String,
        /// None when a candidate is out without a known end.
        resume_at: Option<Timestamp>,
    },
}

struct Excluded {
    reason: String,
    until: Option<Timestamp>,
}

struct Free<'a> {
    account: &'a Account,
    load: Option<i64>,
}

/// The time of day in UTC, as the words of a reason show it.
#[must_use]
pub fn clock_text(at: Timestamp) -> String {
    let text = at.to_string();
    format!("{} UTC", text.get(11..16).unwrap_or_default())
}

/// Picks the account of a run, or says why none is free.
#[must_use]
pub fn choose_account(input: &AccountChoiceInput<'_>) -> AccountChoice {
    let slugs: Vec<&str> = if input.allowed.slugs.is_empty() {
        input.accounts.iter().map(|a| a.slug().as_str()).collect()
    } else {
        input.allowed.slugs.iter().map(String::as_str).collect()
    };
    let from = if input.allowed.slugs.is_empty() {
        String::from("all accounts")
    } else {
        input
            .allowed
            .from
            .clone()
            .unwrap_or_else(|| String::from("setting"))
    };
    let unknown = AccountState::default();
    let mut free: Vec<Free<'_>> = Vec::new();
    let mut excluded: Vec<String> = Vec::new();
    let mut resumes: Vec<Timestamp> = Vec::new();
    let mut all_timed = true;
    for slug in slugs {
        let Some(account) = input.accounts.iter().find(|a| a.slug().as_str() == slug) else {
            excluded.push(format!("{slug}: does not exist"));
            all_timed = false;
            continue;
        };
        let state = input.states.get(&account.id()).unwrap_or(&unknown);
        match excluded_reason(account, state, input) {
            Some(out) => {
                excluded.push(format!("{slug}: {}", out.reason));
                match out.until {
                    Some(until) => resumes.push(until),
                    None => all_timed = false,
                }
            }
            None => free.push(Free {
                account,
                load: load(state),
            }),
        }
    }
    if free.is_empty() {
        let resume_at = if all_timed {
            resumes.iter().min().copied()
        } else {
            None
        };
        let resume = resume_at.map_or_else(String::new, |at| {
            format!(" - free again from {}", clock_text(at))
        });
        let why = if excluded.is_empty() {
            String::from("none allowed")
        } else {
            excluded.join("; ")
        };
        return AccountChoice::Waiting {
            reason: format!("no account free ({why}){resume}"),
            resume_at,
        };
    }
    // fallback accounts count only when no regular one is free
    let regular = free.iter().filter(|f| !f.account.fallback()).count();
    let pool: Vec<&Free<'_>> = free
        .iter()
        .filter(|f| regular == 0 || !f.account.fallback())
        .collect();
    let pick = match input.strategy {
        AccountStrategy::Order => pool[0],
        AccountStrategy::MostFree => least_loaded(&pool),
    };
    let why = pick_reason(&from, pick, &pool, input.strategy, &excluded, regular == 0);
    let snapshot = input
        .states
        .get(&pick.account.id())
        .map_or_else(UsageSnapshot::default, |state| state.usage);
    let slug = pick.account.slug().to_string();
    AccountChoice::Picked {
        account: pick.account.id(),
        why,
        slug,
        snapshot,
    }
}

/// The reason text of a pick: where the list came from, the loads and what was skipped.
fn pick_reason(
    from: &str,
    pick: &Free<'_>,
    pool: &[&Free<'_>],
    strategy: AccountStrategy,
    excluded: &[String],
    no_regular: bool,
) -> String {
    let loads = if pool.len() > 1 {
        let parts: Vec<String> = pool
            .iter()
            .map(|f| {
                let load = f
                    .load
                    .map_or_else(|| String::from("?"), |load| format!("{load} %"));
                format!("{} ({load})", f.account.slug())
            })
            .collect();
        format!(": {}", parts.join(", "))
    } else {
        String::new()
    };
    let strategy = match (pool.len() > 1, strategy) {
        (false, _) => "",
        (true, AccountStrategy::Order) => ", first free",
        (true, AccountStrategy::MostFree) => ", most free capacity",
    };
    let fallback = if no_regular && pick.account.fallback() {
        ", fallback (no other free)"
    } else {
        ""
    };
    let skipped = if excluded.is_empty() {
        String::new()
    } else {
        format!("; skipped {}", excluded.join("; "))
    };
    format!(
        "{from} -> {}{fallback}{strategy}{loads}{skipped}",
        pick.account.slug()
    )
}

fn limit_of(own: Option<crate::Limit>, default: i64) -> i64 {
    own.map_or(default, |limit| i64::from(limit.percent()))
}

fn excluded_reason(
    account: &Account,
    state: &AccountState,
    input: &AccountChoiceInput<'_>,
) -> Option<Excluded> {
    if !account.enabled() {
        return Some(Excluded {
            reason: String::from("disabled"),
            until: None,
        });
    }
    if let Some(until) = state.paused_until.filter(|until| *until > input.now) {
        return Some(Excluded {
            reason: format!("paused until {}", clock_text(until)),
            until: Some(until),
        });
    }
    if state.logged_in == Some(false) {
        return Some(Excluded {
            reason: String::from("not logged in"),
            until: None,
        });
    }
    // the account's own ceiling comes before the dispatcher's default
    let five = limit_of(account.limit_five_hour(), input.limit_percent);
    let week = limit_of(account.limit_week(), input.limit_percent);
    let mut full: Vec<(String, Option<Timestamp>)> = Vec::new();
    if let Some(used) = state.usage.five_hour.filter(|used| *used >= five) {
        let own = if account.limit_five_hour().is_some() {
            format!(", limit {five} %")
        } else {
            String::new()
        };
        full.push((
            format!("5 h {used} %{own}"),
            state.usage.five_hour_resets_at,
        ));
    }
    if let Some(used) = state.usage.seven_day.filter(|used| *used >= week) {
        let own = if account.limit_week().is_some() {
            format!(", limit {week} %")
        } else {
            String::new()
        };
        full.push((
            format!("week {used} %{own}"),
            state.usage.seven_day_resets_at,
        ));
    }
    let Some(first) = full.first() else {
        return tight(account, state, input, five, week);
    };
    let until = latest_reset(&full, input.now);
    let ends = until.map_or_else(String::new, |at| format!(" until {}", clock_text(at)));
    Some(Excluded {
        reason: format!("full ({}){ends}", first.0),
        until,
    })
}

/// Free only once every window has reset: the latest reset; a missing or past one is unknown.
fn latest_reset(windows: &[(String, Option<Timestamp>)], now: Timestamp) -> Option<Timestamp> {
    let mut latest = None;
    for (_, resets_at) in windows {
        let at = (*resets_at).filter(|at| *at > now)?;
        latest = latest.max(Some(at));
    }
    latest
}

/// Tight: use plus the expected use of this run and of runs under way passes the ceiling.
fn tight(
    account: &Account,
    state: &AccountState,
    input: &AccountChoiceInput<'_>,
    five: i64,
    week: i64,
) -> Option<Excluded> {
    let need = input.reserve.plus(
        input
            .pending
            .get(&account.id())
            .copied()
            .unwrap_or_default(),
    );
    let grace = u32::try_from(input.reset_grace_minutes.max(0)).unwrap_or(u32::MAX);
    let grace_end = input.now.checked_add(MINUTE * grace)?;
    let resets_soon = state
        .usage
        .five_hour_resets_at
        .is_some_and(|at| at > input.now && at <= grace_end);
    let mut tight: Vec<(String, Option<Timestamp>)> = Vec::new();
    if let Some(used) = state.usage.five_hour
        && need.five_hour > 0
        && used + need.five_hour > five
        && !resets_soon
    {
        tight.push((
            format!(
                "5 h {used} % + {} % expected, limit {five} %",
                need.five_hour
            ),
            state.usage.five_hour_resets_at,
        ));
    }
    if let Some(used) = state.usage.seven_day
        && need.week > 0
        && used + need.week > week
    {
        tight.push((
            format!("week {used} % + {} % expected, limit {week} %", need.week),
            state.usage.seven_day_resets_at,
        ));
    }
    let first = tight.first()?;
    Some(Excluded {
        reason: format!("tight ({})", first.0),
        until: latest_reset(&tight, input.now),
    })
}

/// The tighter window; none when unknown.
fn load(state: &AccountState) -> Option<i64> {
    match (state.usage.five_hour, state.usage.seven_day) {
        (Some(five), Some(week)) => Some(five.max(week)),
        (five, week) => five.or(week),
    }
}

/// The lowest known load; unknown ones count behind, ties keep the order.
fn least_loaded<'a, 'b>(pool: &[&'a Free<'b>]) -> &'a Free<'b> {
    let mut best = pool[0];
    for next in &pool[1..] {
        let Some(load) = next.load else { continue };
        if best.load.is_none_or(|current| load < current) {
            best = next;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountName, ConfigDir, Limit, Version};

    const NOW: i64 = 1_800_000_000_000;
    const HOUR: i64 = 3_600_000;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis)
    }

    fn account(n: u128, slug: &str, fallback: bool, limits: (Option<i64>, Option<i64>)) -> Account {
        let limit = |value: Option<i64>| value.map(|v| Limit::parse(v, "window").unwrap());
        Account::restore(
            Id::from_uuid(uuid::Uuid::from_u128(n)),
            AccountSlug::parse(slug).unwrap(),
            AccountName::parse(slug).unwrap(),
            Some(ConfigDir::parse("/a").unwrap()),
            true,
            fallback,
            0,
            (limit(limits.0), limit(limits.1)),
            None,
            (at(0), at(0)),
            Version::INITIAL,
        )
    }

    fn plain(n: u128, slug: &str) -> Account {
        account(n, slug, false, (None, None))
    }

    fn used(five: Option<i64>, week: Option<i64>) -> AccountState {
        AccountState {
            usage: UsageSnapshot {
                five_hour: five,
                seven_day: week,
                ..UsageSnapshot::default()
            },
            ..AccountState::default()
        }
    }

    struct Case {
        allowed: AllowedAccounts,
        accounts: Vec<Account>,
        states: HashMap<Id<Account>, AccountState>,
        pending: HashMap<Id<Account>, AccountReserve>,
        strategy: AccountStrategy,
        reserve: AccountReserve,
    }

    impl Case {
        fn new(accounts: Vec<Account>) -> Self {
            Self {
                allowed: AllowedAccounts::default(),
                accounts,
                states: HashMap::new(),
                pending: HashMap::new(),
                strategy: AccountStrategy::MostFree,
                reserve: AccountReserve::default(),
            }
        }

        fn state(&mut self, n: u128, state: AccountState) {
            self.states
                .insert(Id::from_uuid(uuid::Uuid::from_u128(n)), state);
        }

        fn choose(&self) -> AccountChoice {
            choose_account(&AccountChoiceInput {
                allowed: &self.allowed,
                accounts: &self.accounts,
                states: &self.states,
                strategy: self.strategy,
                limit_percent: 90,
                now: at(NOW),
                reserve: self.reserve,
                pending: &self.pending,
                reset_grace_minutes: 30,
            })
        }
    }

    fn picked(choice: &AccountChoice) -> &str {
        match choice {
            AccountChoice::Picked { slug, .. } => slug,
            AccountChoice::Waiting { reason, .. } => panic!("waiting: {reason}"),
        }
    }

    fn waiting(choice: &AccountChoice) -> (&str, Option<Timestamp>) {
        match choice {
            AccountChoice::Waiting { reason, resume_at } => (reason, *resume_at),
            AccountChoice::Picked { why, .. } => panic!("picked: {why}"),
        }
    }

    #[test]
    fn the_least_loaded_account_wins_and_unknown_counts_behind() {
        let mut case = Case::new(vec![plain(1, "a"), plain(2, "b"), plain(3, "c")]);
        case.state(1, used(Some(60), Some(10)));
        case.state(2, used(Some(20), Some(30)));
        case.state(3, used(None, None));
        let choice = case.choose();
        assert_eq!(picked(&choice), "b");
        let AccountChoice::Picked { why, snapshot, .. } = choice else {
            panic!("no pick");
        };
        assert_eq!(
            why,
            "all accounts -> b, most free capacity: a (60 %), b (30 %), c (?)"
        );
        assert_eq!(snapshot.five_hour, Some(20));
    }

    #[test]
    fn ties_and_unknown_loads_keep_the_order() {
        let mut case = Case::new(vec![plain(1, "a"), plain(2, "b")]);
        case.state(1, used(Some(40), None));
        case.state(2, used(Some(40), None));
        assert_eq!(picked(&case.choose()), "a");
        let case = Case::new(vec![plain(1, "a"), plain(2, "b")]);
        assert_eq!(picked(&case.choose()), "a");
    }

    #[test]
    fn the_order_strategy_takes_the_first_free_of_the_allowed_list() {
        let mut case = Case::new(vec![plain(1, "a"), plain(2, "b"), plain(3, "c")]);
        case.strategy = AccountStrategy::Order;
        case.allowed = AllowedAccounts {
            slugs: vec![String::from("c"), String::from("a")],
            from: Some(String::from("profile large")),
        };
        case.state(3, used(Some(80), None));
        case.state(1, used(Some(1), None));
        let AccountChoice::Picked { why, .. } = case.choose() else {
            panic!("no pick");
        };
        assert_eq!(why, "profile large -> c, first free: c (80 %), a (1 %)");
    }

    #[test]
    fn only_the_allowed_accounts_count_and_unknown_ones_wait() {
        let mut case = Case::new(vec![plain(1, "a"), plain(2, "b")]);
        case.allowed = AllowedAccounts {
            slugs: vec![String::from("b")],
            from: None,
        };
        assert_eq!(picked(&case.choose()), "b");
        let AccountChoice::Picked { why, .. } = case.choose() else {
            panic!("no pick");
        };
        assert_eq!(why, "setting -> b");
        case.allowed.slugs = vec![String::from("zed")];
        let choice = case.choose();
        assert_eq!(
            waiting(&choice),
            ("no account free (zed: does not exist)", None)
        );
    }

    #[test]
    fn disabled_logged_out_and_paused_accounts_are_skipped() {
        let off = Account::restore(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            AccountSlug::parse("a").unwrap(),
            AccountName::parse("a").unwrap(),
            None,
            false,
            false,
            0,
            (None, None),
            None,
            (at(0), at(0)),
            Version::INITIAL,
        );
        let mut case = Case::new(vec![off, plain(2, "b"), plain(3, "c"), plain(4, "d")]);
        case.state(
            2,
            AccountState {
                logged_in: Some(false),
                ..AccountState::default()
            },
        );
        case.state(
            3,
            AccountState {
                paused_until: Some(at(NOW + HOUR)),
                ..AccountState::default()
            },
        );
        let AccountChoice::Picked { why, .. } = case.choose() else {
            panic!("no pick");
        };
        assert!(why.starts_with(
            "all accounts -> d; skipped a: disabled; b: not logged in; c: paused until "
        ));
        // a pause that ended no longer counts
        case.state(
            3,
            AccountState {
                paused_until: Some(at(NOW)),
                ..AccountState::default()
            },
        );
        assert_eq!(picked(&case.choose()), "c");
    }

    #[test]
    fn a_window_at_the_ceiling_is_full_and_names_when_it_resets() {
        let mut case = Case::new(vec![plain(1, "a")]);
        case.state(
            1,
            AccountState {
                usage: UsageSnapshot {
                    five_hour: Some(90),
                    five_hour_resets_at: Some(at(NOW + HOUR)),
                    ..UsageSnapshot::default()
                },
                ..AccountState::default()
            },
        );
        let choice = case.choose();
        let (reason, resume) = waiting(&choice);
        assert!(reason.starts_with("no account free (a: full (5 h 90 %) until "));
        assert!(reason.contains(" - free again from "));
        assert_eq!(resume, Some(at(NOW + HOUR)));
        case.state(1, used(Some(89), Some(89)));
        assert_eq!(picked(&case.choose()), "a");
    }

    #[test]
    fn the_own_ceiling_of_an_account_beats_the_default_and_is_named() {
        let mut case = Case::new(vec![account(1, "a", false, (Some(50), Some(95)))]);
        case.state(1, used(Some(60), Some(10)));
        let choice = case.choose();
        assert!(waiting(&choice).0.contains("full (5 h 60 %, limit 50 %)"));
        // the own ceiling may also be higher than the default
        case.state(1, used(Some(10), Some(92)));
        assert_eq!(picked(&case.choose()), "a");
    }

    #[test]
    fn both_full_windows_must_reset_and_one_unknown_reset_is_unknown() {
        let mut case = Case::new(vec![plain(1, "a")]);
        case.state(
            1,
            AccountState {
                usage: UsageSnapshot {
                    five_hour: Some(95),
                    seven_day: Some(99),
                    five_hour_resets_at: Some(at(NOW + HOUR)),
                    seven_day_resets_at: Some(at(NOW + 5 * HOUR)),
                },
                ..AccountState::default()
            },
        );
        assert_eq!(waiting(&case.choose()).1, Some(at(NOW + 5 * HOUR)));
        case.state(
            1,
            AccountState {
                usage: UsageSnapshot {
                    five_hour: Some(95),
                    seven_day: Some(99),
                    five_hour_resets_at: Some(at(NOW + HOUR)),
                    seven_day_resets_at: None,
                },
                ..AccountState::default()
            },
        );
        assert_eq!(waiting(&case.choose()).1, None);
        // a reset in the past is as good as none
        case.state(
            1,
            AccountState {
                usage: UsageSnapshot {
                    five_hour: Some(95),
                    five_hour_resets_at: Some(at(NOW - HOUR)),
                    ..UsageSnapshot::default()
                },
                ..AccountState::default()
            },
        );
        assert_eq!(waiting(&case.choose()).1, None);
    }

    #[test]
    fn the_reserve_makes_an_account_tight_before_it_is_full() {
        let mut case = Case::new(vec![plain(1, "a")]);
        case.reserve = AccountReserve {
            five_hour: 10,
            week: 2,
        };
        case.state(1, used(Some(81), Some(10)));
        let choice = case.choose();
        assert!(
            waiting(&choice)
                .0
                .contains("tight (5 h 81 % + 10 % expected, limit 90 %)")
        );
        case.state(1, used(Some(80), Some(89)));
        assert!(
            waiting(&case.choose())
                .0
                .contains("tight (week 89 % + 2 % expected, limit 90 %)")
        );
        case.state(1, used(Some(80), Some(88)));
        assert_eq!(picked(&case.choose()), "a");
    }

    #[test]
    fn unknown_usage_is_never_tight() {
        let mut case = Case::new(vec![plain(1, "a")]);
        case.reserve = AccountReserve {
            five_hour: 50,
            week: 50,
        };
        case.state(1, used(None, None));
        assert_eq!(picked(&case.choose()), "a");
    }

    #[test]
    fn the_reserve_of_runs_under_way_counts_too() {
        let mut case = Case::new(vec![plain(1, "a")]);
        case.reserve = AccountReserve {
            five_hour: 5,
            week: 0,
        };
        case.state(1, used(Some(70), Some(0)));
        assert_eq!(picked(&case.choose()), "a");
        case.pending.insert(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            AccountReserve {
                five_hour: 20,
                week: 0,
            },
        );
        assert!(
            waiting(&case.choose())
                .0
                .contains("tight (5 h 70 % + 25 % expected")
        );
    }

    #[test]
    fn a_five_hour_window_that_resets_within_the_grace_ignores_the_reserve() {
        let mut case = Case::new(vec![plain(1, "a")]);
        case.reserve = AccountReserve {
            five_hour: 30,
            week: 30,
        };
        let state = |reset: i64, week: i64| AccountState {
            usage: UsageSnapshot {
                five_hour: Some(80),
                seven_day: Some(week),
                five_hour_resets_at: Some(at(reset)),
                ..UsageSnapshot::default()
            },
            ..AccountState::default()
        };
        case.state(1, state(NOW + 30 * 60_000, 10));
        assert_eq!(picked(&case.choose()), "a");
        // one minute beyond the grace the reserve counts again
        case.state(1, state(NOW + 31 * 60_000, 10));
        assert!(waiting(&case.choose()).0.contains("tight (5 h"));
        // the week knows no grace
        case.state(1, state(NOW + 60_000, 70));
        assert!(waiting(&case.choose()).0.contains("tight (week 70 %"));
    }

    #[test]
    fn a_tight_account_is_free_again_when_its_windows_reset() {
        let mut case = Case::new(vec![plain(1, "a")]);
        case.reserve = AccountReserve {
            five_hour: 20,
            week: 0,
        };
        case.state(
            1,
            AccountState {
                usage: UsageSnapshot {
                    five_hour: Some(80),
                    five_hour_resets_at: Some(at(NOW + 2 * HOUR)),
                    ..UsageSnapshot::default()
                },
                ..AccountState::default()
            },
        );
        assert_eq!(waiting(&case.choose()).1, Some(at(NOW + 2 * HOUR)));
    }

    #[test]
    fn fallback_accounts_serve_only_when_no_regular_account_is_free() {
        let mut case = Case::new(vec![
            account(1, "spare", true, (None, None)),
            plain(2, "main"),
        ]);
        case.state(1, used(Some(0), None));
        case.state(2, used(Some(50), None));
        assert_eq!(picked(&case.choose()), "main");
        case.state(2, used(Some(95), None));
        let AccountChoice::Picked { why, .. } = case.choose() else {
            panic!("no pick");
        };
        assert!(
            why.starts_with("all accounts -> spare, fallback (no other free); skipped main: full")
        );
    }

    #[test]
    fn the_earliest_end_among_timed_exclusions_is_the_resume_time() {
        let mut case = Case::new(vec![plain(1, "a"), plain(2, "b")]);
        let paused = |until: i64| AccountState {
            paused_until: Some(at(until)),
            ..AccountState::default()
        };
        case.state(1, paused(NOW + 3 * HOUR));
        case.state(2, paused(NOW + HOUR));
        assert_eq!(waiting(&case.choose()).1, Some(at(NOW + HOUR)));
        // an account out without an end makes the resume time unknown
        case.accounts[1] = Account::restore(
            Id::from_uuid(uuid::Uuid::from_u128(2)),
            AccountSlug::parse("b").unwrap(),
            AccountName::parse("b").unwrap(),
            None,
            false,
            false,
            0,
            (None, None),
            None,
            (at(0), at(0)),
            Version::INITIAL,
        );
        let choice = case.choose();
        let (reason, resume) = waiting(&choice);
        assert_eq!(resume, None);
        assert!(!reason.contains("free again"));
    }

    #[test]
    fn no_accounts_at_all_means_none_allowed() {
        let case = Case::new(Vec::new());
        assert_eq!(
            waiting(&case.choose()),
            ("no account free (none allowed)", None)
        );
    }

    fn config_with_accounts() -> DispatcherConfig {
        let mut config = DispatcherConfig::default();
        config.profiles[0].accounts = vec![String::from("p1")];
        config.rules[0].accounts = vec![String::from("r1"), String::from("r2")];
        config
    }

    fn choice_of(
        config: &DispatcherConfig,
        reason: ProfileReason,
        rule: Option<usize>,
    ) -> ProfileChoice {
        ProfileChoice {
            profile: config.profiles[0].name.clone(),
            reason,
            rule,
            why: String::new(),
            escalated_from: None,
        }
    }

    #[test]
    fn an_escalated_profile_brings_its_own_accounts_instead_of_the_rule_ones() {
        let config = config_with_accounts();
        let mut ruled = choice_of(&config, ProfileReason::Rule, Some(1));
        ruled.escalated_from = Some(String::from("small"));
        let allowed = allowed_accounts(&config, &ruled, &[]);
        assert_eq!(allowed.slugs, vec![String::from("p1")]);
        let labels = vec![String::from("account:max")];
        assert_eq!(allowed_accounts(&config, &ruled, &labels).slugs, ["max"]);
    }

    #[test]
    fn a_label_beats_the_rule_and_the_rule_beats_the_profile() {
        let config = config_with_accounts();
        let ruled = choice_of(&config, ProfileReason::Rule, Some(1));
        let labels = vec![String::from("account:max")];
        assert_eq!(
            allowed_accounts(&config, &ruled, &labels),
            AllowedAccounts {
                slugs: vec![String::from("max")],
                from: Some(String::from("label account:max")),
            }
        );
        assert_eq!(
            allowed_accounts(&config, &ruled, &[]),
            AllowedAccounts {
                slugs: vec![String::from("r1"), String::from("r2")],
                from: Some(String::from("rule 1")),
            }
        );
        let plain = choice_of(&config, ProfileReason::Default, None);
        let allowed = allowed_accounts(&config, &plain, &[]);
        assert_eq!(allowed.slugs, vec![String::from("p1")]);
        assert_eq!(allowed.from, Some(format!("profile {}", plain.profile)));
    }

    #[test]
    fn a_rule_without_accounts_falls_to_the_profile_and_no_list_means_all() {
        let mut config = config_with_accounts();
        config.rules[0].accounts.clear();
        let ruled = choice_of(&config, ProfileReason::Rule, Some(1));
        assert_eq!(allowed_accounts(&config, &ruled, &[]).slugs, vec!["p1"]);
        config.profiles[0].accounts.clear();
        assert_eq!(
            allowed_accounts(&config, &ruled, &[]),
            AllowedAccounts::default()
        );
    }

    #[test]
    fn a_bad_account_label_is_ignored_here_and_a_blank_one_is_none() {
        let config = config_with_accounts();
        let plain = choice_of(&config, ProfileReason::Default, None);
        let bad = vec![String::from("account:Max Private")];
        assert_eq!(allowed_accounts(&config, &plain, &bad).slugs, vec!["p1"]);
        assert_eq!(account_label(&bad), Some("Max Private"));
        assert_eq!(account_label(&[String::from("account: ")]), None);
    }

    #[test]
    fn clock_text_shows_utc_hours_and_minutes() {
        assert_eq!(clock_text(at(1_800_000_000_000)), "08:00 UTC");
    }
}
