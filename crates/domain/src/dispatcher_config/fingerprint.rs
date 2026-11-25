use std::collections::BTreeMap;
use std::fmt::Write;

use sha2::{Digest, Sha256};

use super::config::DispatcherConfig;
use super::values::PromptKind;

/// Hex characters of a hash: short enough to show, long enough against collisions.
pub const FINGERPRINT_HASH_LEN: usize = 16;

/// Canonical JSON with sorted keys, so the order of keys never changes a hash.
enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Text(String),
    List(Vec<Json>),
    Map(BTreeMap<String, Json>),
}

fn text(value: &str) -> Json {
    Json::Text(value.to_owned())
}

fn optional(value: Option<&str>) -> Json {
    value.map_or(Json::Null, text)
}

fn texts(values: &[String]) -> Json {
    Json::List(values.iter().map(|value| text(value)).collect())
}

fn map<const N: usize>(entries: [(&str, Json); N]) -> Json {
    Json::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

impl Json {
    fn render(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Int(value) => out.push_str(&value.to_string()),
            Self::Text(value) => quote(value, out),
            Self::List(items) => {
                out.push('[');
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        out.push(',');
                    }
                    item.render(out);
                }
                out.push(']');
            }
            Self::Map(entries) => {
                out.push('{');
                for (at, (key, value)) in entries.iter().enumerate() {
                    if at > 0 {
                        out.push(',');
                    }
                    quote(key, out);
                    out.push(':');
                    value.render(out);
                }
                out.push('}');
            }
        }
    }
}

fn quote(value: &str, out: &mut String) {
    out.push('"');
    for letter in value.chars() {
        match letter {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// What a run is told by its configuration, and the short hash that names that version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub hash: String,
    pub content: String,
}

fn with_accounts(mut entry: Json, accounts: &[String]) -> Json {
    if let (Json::Map(fields), false) = (&mut entry, accounts.is_empty()) {
        fields.insert(String::from("accounts"), texts(accounts));
    }
    entry
}

fn profiles_json(config: &DispatcherConfig) -> Json {
    let entries = config.profiles.iter().map(|profile| {
        let entry = map([
            ("model", optional(profile.model.as_deref())),
            ("effort", optional(profile.effort.as_deref())),
            ("maxTurns", Json::Int(profile.max_turns)),
            ("timeoutMinutes", Json::Int(profile.timeout_minutes)),
        ]);
        (
            profile.name.clone(),
            with_accounts(entry, &profile.accounts),
        )
    });
    Json::Map(entries.collect())
}

fn rules_json(config: &DispatcherConfig) -> Json {
    let rules = config.rules.iter().map(|rule| {
        let when = &rule.when;
        let mut conditions = BTreeMap::new();
        if let Some(labels) = &when.labels {
            conditions.insert(String::from("labels"), texts(labels));
        }
        if let Some(priorities) = &when.priorities {
            conditions.insert(String::from("priority"), texts(priorities));
        }
        if let Some(files) = when.files_min {
            conditions.insert(String::from("filesMin"), Json::Int(files));
        }
        if let Some(chars) = when.plan_min_chars {
            conditions.insert(String::from("planMinChars"), Json::Int(chars));
        }
        let entry = map([
            ("profile", text(&rule.profile)),
            ("when", Json::Map(conditions)),
        ]);
        with_accounts(entry, &rule.accounts)
    });
    Json::List(rules.collect())
}

fn claude_json(config: &DispatcherConfig) -> Json {
    let claude = &config.claude;
    map([
        ("model", optional(claude.model.as_deref())),
        ("effort", optional(claude.effort.as_deref())),
        ("maxTurns", Json::Int(claude.max_turns)),
        ("timeoutMinutes", Json::Int(claude.timeout_minutes)),
        (
            "permissionMode",
            optional(claude.permission_mode.as_deref()),
        ),
        ("allowedTools", texts(&claude.allowed_tools)),
        ("extraArgs", texts(&claude.extra_args)),
    ])
}

impl DispatcherConfig {
    /// The values that change what a run does; window, wip and labels leave the hash alone.
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        let prompts = [
            PromptKind::Work,
            PromptKind::Plan,
            PromptKind::Research,
            PromptKind::Merge,
        ]
        .into_iter()
        .map(|kind| (kind.as_str().to_owned(), optional(self.prompts.get(kind))));
        let setup = self
            .worktree_setup
            .iter()
            .map(|step| map([("cwd", text(&step.cwd)), ("run", text(&step.run))]));
        let whole = map([
            ("profiles", profiles_json(self)),
            ("defaultProfile", text(&self.default_profile)),
            ("rules", rules_json(self)),
            ("escalate", Json::Bool(self.escalate)),
            ("ladder", texts(&self.ladder)),
            ("maxAttempts", Json::Int(self.max_attempts)),
            ("allowFable", Json::Bool(self.allow_fable)),
            ("promptExtra", text(&self.prompt_extra)),
            ("prompts", Json::Map(prompts.collect())),
            ("mergeStrategy", text(self.merge_strategy.as_str())),
            ("reviewBeforeMerge", Json::Bool(self.review_before_merge)),
            ("joinWorktrees", text(self.join_worktrees.as_str())),
            ("worktreeSetup", Json::List(setup.collect())),
            (
                "worktreeSetupTimeoutMinutes",
                Json::Int(self.worktree_setup_timeout_minutes),
            ),
            ("claude", claude_json(self)),
            (
                "research",
                map([("profile", optional(self.research.profile.as_deref()))]),
            ),
            (
                "planning",
                map([("profile", optional(self.planning.profile.as_deref()))]),
            ),
        ]);
        let mut content = String::new();
        whole.render(&mut content);
        let mut hash = String::with_capacity(64);
        for byte in Sha256::digest(content.as_bytes()) {
            let _ = write!(hash, "{byte:02x}");
        }
        hash.truncate(FINGERPRINT_HASH_LEN);
        Fingerprint { hash, content }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JoinWorktrees, MergeStrategy, WorktreeSetupStep};

    type Change = Box<dyn Fn(&mut DispatcherConfig)>;

    fn hash(change: impl FnOnce(&mut DispatcherConfig)) -> String {
        let mut config = DispatcherConfig::default();
        change(&mut config);
        config.fingerprint().hash
    }

    #[test]
    fn the_hash_is_sixteen_lowercase_hex_characters_and_stable() {
        let first = DispatcherConfig::default().fingerprint();
        assert_eq!(first.hash.len(), FINGERPRINT_HASH_LEN);
        assert!(
            first
                .hash
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(first, DispatcherConfig::default().fingerprint());
        assert!(
            first
                .content
                .starts_with("{\"allowFable\":false,\"claude\":{")
        );
    }

    #[test]
    fn what_a_run_feels_changes_the_hash() {
        let base = hash(|_| {});
        let own = |config: &mut DispatcherConfig| config.prompts.work = Some(String::from("own"));
        let changes: Vec<Change> = vec![
            Box::new(|config| config.profiles[0].max_turns = 41),
            Box::new(|config| config.profiles[0].accounts = vec![String::from("max")]),
            Box::new(|config| config.default_profile = String::from("large")),
            Box::new(|config| config.rules[0].profile = String::from("large")),
            Box::new(|config| config.rules[0].accounts = vec![String::from("max")]),
            Box::new(|config| config.escalate = false),
            Box::new(|config| config.ladder.reverse()),
            Box::new(|config| config.max_attempts = 3),
            Box::new(|config| config.allow_fable = true),
            Box::new(|config| config.prompt_extra = String::from("be brief")),
            Box::new(own),
            Box::new(|config| config.merge_strategy = MergeStrategy::Squash),
            Box::new(|config| config.review_before_merge = false),
            Box::new(|config| config.join_worktrees = JoinWorktrees::SameSpec),
            Box::new(|config| config.worktree_setup_timeout_minutes = 5),
            Box::new(|config| config.claude.model = None),
            Box::new(|config| config.claude.allowed_tools.truncate(3)),
            Box::new(|config| config.claude.extra_args = vec![String::from("--x")]),
            Box::new(|config| config.research.profile = Some(String::from("large"))),
            Box::new(|config| config.planning.profile = Some(String::from("large"))),
            Box::new(|config| {
                config.worktree_setup = vec![WorktreeSetupStep {
                    cwd: String::from("."),
                    run: String::from("npm ci"),
                }];
            }),
        ];
        for (at, change) in changes.iter().enumerate() {
            assert_ne!(hash(change), base, "change {at}");
        }
    }

    #[test]
    fn what_a_run_does_not_feel_leaves_the_hash_alone() {
        let base = hash(|_| {});
        let changes: Vec<Change> = vec![
            Box::new(|config| config.enabled = true),
            Box::new(|config| config.wip = 4),
            Box::new(|config| config.opt_in_label = None),
            Box::new(|config| config.agent_prefix = String::from("bot-")),
            Box::new(|config| config.stale_minutes = 999),
            Box::new(|config| config.account_limit_percent = 70),
            Box::new(|config| config.claude.command = String::from("/usr/bin/claude")),
            Box::new(|config| config.claude.use_subscription = false),
            Box::new(|config| config.research.auto = true),
            Box::new(|config| config.autopilot.enabled = true),
        ];
        for (at, change) in changes.iter().enumerate() {
            assert_eq!(hash(change), base, "change {at}");
        }
    }

    #[test]
    fn every_prompt_kind_is_rendered_and_null_when_unset() {
        let content = DispatcherConfig::default().fingerprint().content;
        assert!(
            content.contains(r#""prompts":{"merge":null,"plan":null,"research":null,"work":null}"#)
        );
    }

    #[test]
    fn row_ids_do_not_change_the_hash() {
        assert_eq!(
            hash(|config| config.profiles[0].id = Some(uuid::Uuid::from_u128(1))),
            hash(|_| {})
        );
    }

    #[test]
    fn the_order_of_profiles_does_not_change_the_hash() {
        assert_eq!(hash(|config| config.profiles.reverse()), hash(|_| {}));
    }

    #[test]
    fn text_is_escaped_like_json() {
        let mut out = String::new();
        quote("a\"b\\c\n\u{1}\u{e9}", &mut out);
        assert_eq!(out, "\"a\\\"b\\\\c\\n\\u0001\u{e9}\"");
    }
}
