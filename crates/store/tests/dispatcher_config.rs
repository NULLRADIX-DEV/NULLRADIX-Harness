#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::Arc;

use nullradix_app::accounts::AccountRepository;
use nullradix_app::dispatcher_config::DispatcherConfigService;
use nullradix_app::{
    AppError, ChangeFeed, ChangeOp, ChangeSignal, Entity, ErrorKind, SystemClock, UuidV7Generator,
};
use nullradix_domain::{
    Account, AccountName, AccountSlug, ConfigDir, DispatcherConfig, Id, JoinWorktrees,
    MergeStrategy, PromptKind, RuleCondition, TimeWindow, Version, WorktreeSetupStep,
};
use nullradix_store::Store;
use uuid::Uuid;

struct Fixture {
    config: DispatcherConfigService,
    store: Arc<Store>,
    signal: ChangeSignal,
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let config = DispatcherConfigService::new(
        store.clone(),
        store.clone(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    );
    Fixture {
        config,
        store,
        signal,
    }
}

impl Fixture {
    async fn add_account(&self, slug: &str) {
        let account = Account::new(
            Id::from_uuid(Uuid::now_v7()),
            AccountSlug::parse(slug).unwrap(),
            AccountName::parse(slug).unwrap(),
            ConfigDir::from_stored(format!("/accounts/{slug}")),
            1,
            nullradix_app::Clock::now(&SystemClock),
        );
        self.store.insert(&account).await.unwrap();
    }

    async fn ops(&self) -> Vec<(Entity, ChangeOp)> {
        self.store
            .changes_since(0, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|change| (change.entity, change.op))
            .collect()
    }

    async fn current(&self) -> DispatcherConfig {
        self.config.get().await.unwrap().config
    }
}

fn code(error: &AppError) -> (ErrorKind, &'static str) {
    (error.kind(), error.code())
}

fn fields(error: &AppError) -> Vec<String> {
    error
        .fields()
        .iter()
        .map(|issue| issue.field.clone())
        .collect()
}

#[tokio::test]
async fn a_fresh_store_holds_the_default_configuration() {
    let f = fixture().await;
    let view = f.config.get().await.unwrap();
    assert_eq!(view.config, DispatcherConfig::default());
    assert_eq!(view.version, Version::INITIAL);
    assert_eq!(view.hash, DispatcherConfig::default().fingerprint().hash);
    assert!(view.updated_at.as_millis() > 1_767_225_600_000);
}

#[tokio::test]
async fn the_first_read_keeps_the_default_as_a_version_once() {
    let f = fixture().await;
    assert_eq!(f.config.versions().await.unwrap(), vec![]);
    let first = f.config.get().await.unwrap();
    f.config.get().await.unwrap();
    let versions = f.config.versions().await.unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].hash, first.hash);
    assert_eq!(
        versions[0].content,
        DispatcherConfig::default().fingerprint().content
    );
    assert_eq!(f.ops().await, vec![]);
}

#[tokio::test]
async fn an_update_is_stored_versioned_announced_and_read_back() {
    let f = fixture().await;
    let head = f.signal.subscribe();
    let mut draft = f.current().await;
    draft.enabled = true;
    draft.wip = 3;
    draft.window = Some(TimeWindow {
        from: String::from("18:00"),
        to: String::from("07:00"),
    });
    draft.opt_in_label = None;
    draft.merge_strategy = MergeStrategy::Squash;
    draft.join_worktrees = JoinWorktrees::SameSpec;
    draft
        .claude
        .allowed_tools
        .push(String::from("Bash(cargo:*)"));
    draft.claude.extra_args = vec![String::from("--add-dir"), String::from("../x")];
    draft.claude.effort = Some(String::from("high"));
    draft.claude.git_bash_path = Some(String::from("C:/Git/bin/bash.exe"));
    draft.research.profile = Some(String::from("large"));
    draft.planning.profile = Some(String::from("small"));
    draft.autopilot.except_label = None;
    draft.account_strategy = nullradix_domain::AccountStrategy::Order;

    let saved = f
        .config
        .update(draft.clone(), Version::INITIAL)
        .await
        .unwrap();
    assert_eq!(saved.version, Version::from_stored(2));
    assert_eq!(saved.config, draft);
    let read = f.config.get().await.unwrap();
    assert_eq!(read.config, draft);
    assert_eq!(read.version, Version::from_stored(2));
    assert_eq!(read.hash, saved.hash);
    assert_eq!(
        f.ops().await,
        vec![(Entity::DispatcherConfig, ChangeOp::Update)]
    );
    assert!(head.has_changed().unwrap());
}

#[tokio::test]
async fn a_version_row_appears_only_when_the_effective_hash_changes() {
    let f = fixture().await;
    let mut draft = f.current().await;
    draft.wip = 4;
    let view = f
        .config
        .update(draft.clone(), Version::INITIAL)
        .await
        .unwrap();
    assert_eq!(f.config.versions().await.unwrap().len(), 1);
    assert_eq!(view.hash, DispatcherConfig::default().fingerprint().hash);

    draft.profiles[0].max_turns = 41;
    let view = f.config.update(draft.clone(), view.version).await.unwrap();
    let versions = f.config.versions().await.unwrap();
    assert_eq!(versions.len(), 2);
    assert!(versions.iter().any(|version| version.hash == view.hash));

    draft.profiles[0].max_turns = 40;
    f.config.update(draft, view.version).await.unwrap();
    assert_eq!(f.config.versions().await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_draft_that_changes_nothing_keeps_the_version_and_stays_quiet() {
    let f = fixture().await;
    let view = f
        .config
        .update(f.current().await, Version::INITIAL)
        .await
        .unwrap();
    assert_eq!(view.version, Version::INITIAL);
    assert_eq!(f.ops().await, vec![]);
}

#[tokio::test]
async fn a_stale_version_is_refused_and_changes_nothing() {
    let f = fixture().await;
    let mut draft = f.current().await;
    draft.wip = 2;
    f.config
        .update(draft.clone(), Version::INITIAL)
        .await
        .unwrap();
    draft.wip = 5;
    let error = f.config.update(draft, Version::INITIAL).await.unwrap_err();
    assert_eq!(code(&error), (ErrorKind::Conflict, "version-conflict"));
    assert_eq!(f.current().await.wip, 2);
    assert_eq!(f.ops().await.len(), 1);
}

#[tokio::test]
async fn an_invalid_draft_reports_every_field_and_stores_nothing() {
    let f = fixture().await;
    let mut draft = f.current().await;
    draft.wip = -1;
    draft.default_profile = String::from("giant");
    draft.research.wip = 6;
    let error = f.config.update(draft, Version::INITIAL).await.unwrap_err();
    assert_eq!(
        code(&error),
        (ErrorKind::Invalid, "dispatcher-config-invalid")
    );
    assert_eq!(fields(&error), ["wip", "defaultProfile", "research.wip"]);
    assert_eq!(f.current().await, DispatcherConfig::default());
    assert_eq!(f.ops().await, vec![]);
}

#[tokio::test]
async fn accounts_must_exist_and_keep_their_order() {
    let f = fixture().await;
    let mut draft = f.current().await;
    draft.profiles[2].accounts = vec![String::from("max"), String::from("ghost")];
    draft.rules[0].accounts = vec![String::from("nobody")];
    let error = f.config.update(draft, Version::INITIAL).await.unwrap_err();
    assert_eq!(
        code(&error),
        (ErrorKind::Invalid, "dispatcher-config-invalid")
    );
    assert_eq!(
        fields(&error),
        [
            "profiles[2].accounts[0]",
            "profiles[2].accounts[1]",
            "rules[0].accounts[0]"
        ]
    );

    f.add_account("max").await;
    f.add_account("work").await;
    let mut draft = f.current().await;
    draft.profiles[2].accounts = vec![
        String::from("work"),
        String::from("default"),
        String::from("max"),
    ];
    draft.rules[0].accounts = vec![String::from("max")];
    let saved = f
        .config
        .update(draft.clone(), Version::INITIAL)
        .await
        .unwrap();
    assert_eq!(
        saved.config.profiles[2].accounts,
        ["work", "default", "max"]
    );
    let read = f.current().await;
    assert_eq!(read.profiles[2].accounts, ["work", "default", "max"]);
    assert_eq!(read.rules[0].accounts, ["max"]);
    assert_eq!(read, draft);
}

#[tokio::test]
async fn profiles_rules_and_the_ladder_are_replaced_as_a_whole() {
    let f = fixture().await;
    let mut draft = f.current().await;
    draft.profiles.remove(0);
    draft.profiles[0].name = String::from("regular");
    draft.profiles.push(nullradix_domain::Profile {
        name: String::from("huge"),
        model: None,
        effort: None,
        max_turns: 300,
        timeout_minutes: 200,
        accounts: Vec::new(),
    });
    draft.default_profile = String::from("regular");
    draft.ladder = vec![String::from("huge"), String::from("regular")];
    draft.rules = vec![
        nullradix_domain::Rule {
            when: RuleCondition {
                plan_min_chars: Some(2000),
                ..RuleCondition::default()
            },
            profile: String::from("huge"),
            accounts: Vec::new(),
        },
        nullradix_domain::Rule {
            when: RuleCondition {
                labels: Some(vec![String::from("quick")]),
                priorities: Some(vec![String::from("low"), String::from("normal")]),
                ..RuleCondition::default()
            },
            profile: String::from("regular"),
            accounts: Vec::new(),
        },
    ];
    draft.research.profile = Some(String::from("huge"));
    draft.stale_minutes = 230;
    let saved = f
        .config
        .update(draft.clone(), Version::INITIAL)
        .await
        .unwrap();
    assert_eq!(saved.config, draft);
    let read = f.current().await;
    assert_eq!(read, draft);
    assert_eq!(
        read.profiles
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["regular", "large", "huge"]
    );
    assert_eq!(read.ladder, ["huge", "regular"]);
    assert_eq!(read.rules.len(), 2);
}

#[tokio::test]
async fn setup_steps_and_own_templates_round_trip_and_clear() {
    let f = fixture().await;
    let mut draft = f.current().await;
    draft.worktree_setup = vec![
        WorktreeSetupStep {
            cwd: String::from("."),
            run: String::from("npm ci"),
        },
        WorktreeSetupStep {
            cwd: String::from("web/app"),
            run: String::from("npm run build"),
        },
    ];
    draft
        .prompts
        .set(PromptKind::Work, Some(String::from("do the work\nnow")));
    draft
        .prompts
        .set(PromptKind::Merge, Some(String::from("merge it")));
    draft.prompt_extra = String::from("be kind");
    let saved = f
        .config
        .update(draft.clone(), Version::INITIAL)
        .await
        .unwrap();
    let read = f.current().await;
    assert_eq!(read, draft);

    let mut next = read.clone();
    next.worktree_setup.pop();
    next.prompts.set(PromptKind::Work, None);
    next.prompts
        .set(PromptKind::Plan, Some(String::from("plan it")));
    let after = f.config.update(next.clone(), saved.version).await.unwrap();
    assert_eq!(after.version, Version::from_stored(3));
    let read = f.current().await;
    assert_eq!(read, next);
    assert_eq!(read.prompts.entries().len(), 2);
}

#[tokio::test]
async fn removing_an_account_drops_it_from_the_configuration_and_records_the_new_hash() {
    let f = fixture().await;
    f.add_account("max").await;
    let mut draft = f.current().await;
    draft.profiles[2].accounts = vec![String::from("max")];
    let saved = f.config.update(draft, Version::INITIAL).await.unwrap();
    let removed = f.store.list().await.unwrap();
    let max = removed
        .iter()
        .find(|account| account.slug().as_str() == "max")
        .unwrap();
    f.store
        .delete(
            max.id(),
            max.version(),
            nullradix_app::Clock::now(&SystemClock),
        )
        .await
        .unwrap();

    let view = f.config.get().await.unwrap();
    assert_eq!(view.config.profiles[2].accounts, Vec::<String>::new());
    assert_ne!(view.hash, saved.hash);
    assert_eq!(view.version, saved.version);
    assert!(
        f.config
            .versions()
            .await
            .unwrap()
            .iter()
            .any(|v| v.hash == view.hash)
    );
}

#[tokio::test]
async fn racing_updates_let_exactly_one_win() {
    let f = Arc::new(fixture().await);
    let draft = |wip: i64| {
        let f = f.clone();
        async move {
            let mut draft = f.current().await;
            draft.wip = wip;
            f.config.update(draft, Version::INITIAL).await
        }
    };
    let (first, second) = tokio::join!(tokio::spawn(draft(2)), tokio::spawn(draft(3)));
    let results = [first.unwrap(), second.unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let refused = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .unwrap();
    assert_eq!(code(refused), (ErrorKind::Conflict, "version-conflict"));
}
