use std::fmt;

use crate::worktree::slug_from;
use crate::{Id, TextError, Timestamp, Version, required_text};

/// Slug of the account that follows the server's own Claude login.
pub const DEFAULT_SLUG: &str = "default";
pub const ACCOUNT_NAME_MAX: usize = 60;
pub const ACCOUNT_SLUG_MAX: usize = 40;
pub const CONFIG_DIR_MAX: usize = 1000;
/// Start of the long-lived tokens `claude setup-token` prints.
pub const TOKEN_PREFIX: &str = "sk-ant-oat";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccountError {
    #[error(
        "an account id uses 1 to 40 characters of a-z, 0-9 and -, starting with a letter or digit"
    )]
    Slug,
    #[error("the account name {0}")]
    Name(TextError),
    #[error("the folder {0}")]
    ConfigDir(TextError),
    #[error("the limit for {0} must be a whole number from 1 to 100")]
    Limit(&'static str),
    #[error("a run token starts with {TOKEN_PREFIX} and has no spaces")]
    Token,
}

/// Identifier of an account in labels and on the command line, such as `max-private`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountSlug(String);

impl AccountSlug {
    pub fn parse(text: &str) -> Result<Self, AccountError> {
        let valid = (1..=ACCOUNT_SLUG_MAX).contains(&text.len())
            && text.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && text
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        valid
            .then(|| Self(text.to_owned()))
            .ok_or(AccountError::Slug)
    }

    /// Short form of a name, or `account` when nothing usable is left.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        let base = slug_from(name, ACCOUNT_SLUG_MAX);
        Self(if base.is_empty() {
            String::from("account")
        } else {
            base
        })
    }

    /// The slug itself when `taken` allows it, else with `-2`, `-3` and so on.
    #[must_use]
    pub fn unique(&self, taken: impl Fn(&str) -> bool) -> Self {
        if !taken(&self.0) {
            return self.clone();
        }
        let short: String = self.0.chars().take(ACCOUNT_SLUG_MAX - 4).collect();
        let short = short.trim_end_matches('-');
        (2_u32..=u32::MAX)
            .map(|number| format!("{short}-{number}"))
            .find(|candidate| !taken(candidate))
            .map_or_else(|| self.clone(), Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_default(&self) -> bool {
        self.0 == DEFAULT_SLUG
    }
}

impl fmt::Display for AccountSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountName(String);

impl AccountName {
    pub fn parse(text: &str) -> Result<Self, AccountError> {
        required_text(text, ACCOUNT_NAME_MAX)
            .map(|name| Self(name.to_owned()))
            .map_err(AccountError::Name)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Folder of Claude's own files for an account, as typed; `~` stands for the home folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDir(String);

impl ConfigDir {
    pub fn parse(text: &str) -> Result<Self, AccountError> {
        required_text(text, CONFIG_DIR_MAX)
            .map(|dir| Self(dir.to_owned()))
            .map_err(AccountError::ConfigDir)
    }

    /// Trusts a value read back from storage.
    #[must_use]
    pub fn from_stored(text: String) -> Self {
        Self(text)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A usage ceiling in percent; above it the dispatcher leaves the account alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Limit(u8);

impl Limit {
    pub fn parse(value: i64, window: &'static str) -> Result<Self, AccountError> {
        u8::try_from(value)
            .ok()
            .filter(|percent| (1..=100).contains(percent))
            .map(Self)
            .ok_or(AccountError::Limit(window))
    }

    #[must_use]
    pub const fn percent(self) -> u8 {
        self.0
    }
}

/// Long-lived Claude token; it is only ever held in memory and in the secret store.
#[derive(Clone, PartialEq, Eq)]
pub struct RunToken(String);

impl RunToken {
    pub fn parse(text: &str) -> Result<Self, AccountError> {
        let token = text.trim();
        let valid = token.starts_with(TOKEN_PREFIX)
            && token.len() > TOKEN_PREFIX.len()
            && !token.chars().any(char::is_whitespace);
        valid
            .then(|| Self(token.to_owned()))
            .ok_or(AccountError::Token)
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RunToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RunToken(..)")
    }
}

/// A change of an account; left out fields stay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountChanges {
    pub name: Option<AccountName>,
    pub enabled: Option<bool>,
    pub fallback: Option<bool>,
    #[allow(
        clippy::option_option,
        reason = "missing keeps the limit, none removes it"
    )]
    pub limit_five_hour: Option<Option<Limit>>,
    #[allow(
        clippy::option_option,
        reason = "missing keeps the limit, none removes it"
    )]
    pub limit_week: Option<Option<Limit>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    id: Id<Account>,
    slug: AccountSlug,
    name: AccountName,
    config_dir: Option<ConfigDir>,
    enabled: bool,
    fallback: bool,
    position: i64,
    limit_five_hour: Option<Limit>,
    limit_week: Option<Limit>,
    token_ref: Option<String>,
    created_at: Timestamp,
    updated_at: Timestamp,
    version: Version,
}

impl Account {
    #[must_use]
    pub fn new(
        id: Id<Self>,
        slug: AccountSlug,
        name: AccountName,
        config_dir: ConfigDir,
        position: i64,
        now: Timestamp,
    ) -> Self {
        Self {
            id,
            slug,
            name,
            config_dir: Some(config_dir),
            enabled: true,
            fallback: false,
            position,
            limit_five_hour: None,
            limit_week: None,
            token_ref: None,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        }
    }

    /// Rebuilds a stored account; only persistence adapters call this.
    #[must_use]
    #[allow(clippy::too_many_arguments, reason = "mirrors the stored columns")]
    pub fn restore(
        id: Id<Self>,
        slug: AccountSlug,
        name: AccountName,
        config_dir: Option<ConfigDir>,
        enabled: bool,
        fallback: bool,
        position: i64,
        limits: (Option<Limit>, Option<Limit>),
        token_ref: Option<String>,
        times: (Timestamp, Timestamp),
        version: Version,
    ) -> Self {
        Self {
            id,
            slug,
            name,
            config_dir,
            enabled,
            fallback,
            position,
            limit_five_hour: limits.0,
            limit_week: limits.1,
            token_ref,
            created_at: times.0,
            updated_at: times.1,
            version,
        }
    }

    /// The account after `changes`, or `None` when nothing differs.
    #[must_use]
    pub fn changed(&self, changes: AccountChanges, now: Timestamp) -> Option<Self> {
        let mut next = self.clone();
        if let Some(name) = changes.name {
            next.name = name;
        }
        if let Some(enabled) = changes.enabled {
            next.enabled = enabled;
        }
        if let Some(fallback) = changes.fallback {
            next.fallback = fallback;
        }
        if let Some(limit) = changes.limit_five_hour {
            next.limit_five_hour = limit;
        }
        if let Some(limit) = changes.limit_week {
            next.limit_week = limit;
        }
        (next != *self).then(|| next.stamped(now))
    }

    /// The account pointing at a stored token, or at none; `None` when it already does.
    #[must_use]
    pub fn with_token_ref(&self, token_ref: Option<String>, now: Timestamp) -> Option<Self> {
        (token_ref != self.token_ref).then(|| {
            Self {
                token_ref,
                ..self.clone()
            }
            .stamped(now)
        })
    }

    fn stamped(mut self, now: Timestamp) -> Self {
        self.updated_at = now.max(self.updated_at);
        self.version = self.version.next();
        self
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.id
    }

    #[must_use]
    pub const fn slug(&self) -> &AccountSlug {
        &self.slug
    }

    #[must_use]
    pub const fn name(&self) -> &AccountName {
        &self.name
    }

    /// `None` for the default account, which uses the server's own environment.
    #[must_use]
    pub const fn config_dir(&self) -> Option<&ConfigDir> {
        self.config_dir.as_ref()
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn fallback(&self) -> bool {
        self.fallback
    }

    #[must_use]
    pub const fn position(&self) -> i64 {
        self.position
    }

    #[must_use]
    pub const fn limit_five_hour(&self) -> Option<Limit> {
        self.limit_five_hour
    }

    #[must_use]
    pub const fn limit_week(&self) -> Option<Limit> {
        self.limit_week
    }

    /// Key of the token in the secret store, when one is set.
    #[must_use]
    pub fn token_ref(&self) -> Option<&str> {
        self.token_ref.as_deref()
    }

    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }

    #[must_use]
    pub const fn updated_at(&self) -> Timestamp {
        self.updated_at
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    #[must_use]
    pub fn is_default(&self) -> bool {
        self.slug.is_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> Account {
        Account::new(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            AccountSlug::parse("max").unwrap(),
            AccountName::parse("Max").unwrap(),
            ConfigDir::parse("/data/accounts/max").unwrap(),
            1,
            Timestamp::from_millis(5),
        )
    }

    #[test]
    fn slugs_follow_the_legacy_pattern() {
        for good in ["default", "pro-1", "a", "0x"] {
            assert_eq!(AccountSlug::parse(good).unwrap().as_str(), good);
        }
        for bad in ["", "-start", "Boese", "a b", "a_b", &"a".repeat(41)] {
            assert_eq!(AccountSlug::parse(bad), Err(AccountError::Slug), "{bad}");
        }
        assert!(AccountSlug::parse(&"a".repeat(40)).is_ok());
    }

    #[test]
    fn slugs_come_from_names() {
        assert_eq!(
            AccountSlug::from_name("Max (private)").as_str(),
            "max-private"
        );
        assert_eq!(AccountSlug::from_name("Gr\u{fc}n").as_str(), "gruen");
        assert_eq!(AccountSlug::from_name("!!!").as_str(), "account");
        assert_eq!(
            AccountSlug::from_name(&"word ".repeat(20)).as_str().len(),
            39
        );
    }

    #[test]
    fn taken_slugs_get_a_number() {
        let base = AccountSlug::parse("max").unwrap();
        assert_eq!(base.unique(|_| false).as_str(), "max");
        let taken = ["max", "max-2"];
        assert_eq!(base.unique(|slug| taken.contains(&slug)).as_str(), "max-3");
        let long = AccountSlug::parse(&"a".repeat(40)).unwrap();
        let fitted = long.unique(|slug| slug.len() == 40);
        assert_eq!(fitted.as_str().len(), 38);
        assert!(fitted.as_str().ends_with("-2"));
    }

    #[test]
    fn names_and_folders_are_trimmed_and_bounded() {
        assert_eq!(AccountName::parse(" Max ").unwrap().as_str(), "Max");
        assert_eq!(
            AccountName::parse(" "),
            Err(AccountError::Name(TextError::Empty))
        );
        assert!(AccountName::parse(&"x".repeat(60)).is_ok());
        assert_eq!(
            AccountName::parse(&"x".repeat(61)),
            Err(AccountError::Name(TextError::TooLong(60)))
        );
        assert_eq!(ConfigDir::parse(" ~/a ").unwrap().as_str(), "~/a");
        assert_eq!(
            ConfigDir::parse(""),
            Err(AccountError::ConfigDir(TextError::Empty))
        );
    }

    #[test]
    fn limits_are_whole_percentages() {
        assert_eq!(Limit::parse(1, "week").unwrap().percent(), 1);
        assert_eq!(Limit::parse(100, "week").unwrap().percent(), 100);
        for bad in [0, 101, -5, 256, i64::MAX] {
            assert_eq!(Limit::parse(bad, "week"), Err(AccountError::Limit("week")));
        }
    }

    #[test]
    fn tokens_look_like_claude_run_tokens() {
        let token = RunToken::parse("  sk-ant-oat01-abc\n").unwrap();
        assert_eq!(token.expose(), "sk-ant-oat01-abc");
        for bad in [
            "",
            "sk-ant-api03-key",
            "sk-ant-oat01 abc",
            "sk-ant-oat",
            "oat",
        ] {
            assert_eq!(RunToken::parse(bad), Err(AccountError::Token), "{bad}");
        }
    }

    #[test]
    fn tokens_never_show_in_debug_output() {
        let token = RunToken::parse("sk-ant-oat01-secret").unwrap();
        assert_eq!(format!("{token:?}"), "RunToken(..)");
    }

    #[test]
    fn new_accounts_start_enabled_without_limits() {
        let account = account();
        assert!(account.enabled());
        assert!(!account.fallback());
        assert_eq!(account.limit_week(), None);
        assert_eq!(account.version(), Version::INITIAL);
        assert!(!account.is_default());
    }

    #[test]
    fn changes_bump_version_and_time() {
        let original = account();
        let changed = original
            .changed(
                AccountChanges {
                    enabled: Some(false),
                    limit_week: Some(Some(Limit::parse(80, "week").unwrap())),
                    ..AccountChanges::default()
                },
                Timestamp::from_millis(9),
            )
            .unwrap();
        assert!(!changed.enabled());
        assert_eq!(changed.limit_week().map(Limit::percent), Some(80));
        assert_eq!(changed.version(), original.version().next());
        assert_eq!(changed.updated_at(), Timestamp::from_millis(9));
        assert_eq!(changed.created_at(), original.created_at());
    }

    #[test]
    fn removing_a_limit_is_a_change() {
        let limited = account()
            .changed(
                AccountChanges {
                    limit_five_hour: Some(Some(Limit::parse(50, "5 hours").unwrap())),
                    ..AccountChanges::default()
                },
                Timestamp::from_millis(6),
            )
            .unwrap();
        let cleared = limited
            .changed(
                AccountChanges {
                    limit_five_hour: Some(None),
                    ..AccountChanges::default()
                },
                Timestamp::from_millis(7),
            )
            .unwrap();
        assert_eq!(cleared.limit_five_hour(), None);
    }

    #[test]
    fn unchanged_values_change_nothing() {
        let original = account();
        let same = AccountChanges {
            name: Some(AccountName::parse("Max").unwrap()),
            enabled: Some(true),
            fallback: Some(false),
            limit_five_hour: Some(None),
            limit_week: None,
        };
        assert_eq!(original.changed(same, Timestamp::from_millis(9)), None);
    }

    #[test]
    fn the_clock_never_moves_updated_at_backwards() {
        let changed = account()
            .changed(
                AccountChanges {
                    fallback: Some(true),
                    ..AccountChanges::default()
                },
                Timestamp::from_millis(1),
            )
            .unwrap();
        assert_eq!(changed.updated_at(), Timestamp::from_millis(5));
    }

    #[test]
    fn token_references_are_set_and_cleared() {
        let original = account();
        let set = original
            .with_token_ref(
                Some(String::from("account-token/1")),
                Timestamp::from_millis(6),
            )
            .unwrap();
        assert_eq!(set.token_ref(), Some("account-token/1"));
        assert_eq!(set.version(), original.version().next());
        assert_eq!(
            set.with_token_ref(
                Some(String::from("account-token/1")),
                Timestamp::from_millis(7)
            ),
            None
        );
        let cleared = set.with_token_ref(None, Timestamp::from_millis(8)).unwrap();
        assert_eq!(cleared.token_ref(), None);
    }
}
