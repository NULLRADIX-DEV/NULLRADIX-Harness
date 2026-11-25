use std::collections::HashMap;

use async_trait::async_trait;
use nullradix_app::dispatcher_config::{
    CONFIG_ENTITY_ID, ConfigSave, ConfigVersion, DispatcherConfigRepository, SaveOutcome,
    StoredConfig,
};
use nullradix_app::{AppError, Change, ChangeOp, Entity, FieldProblem, IdGenerator};
use nullradix_domain::{
    AccountStrategy, AutopilotSettings, ClaudeSettings, DispatcherConfig, Fingerprint,
    JoinWorktrees, MergeStrategy, PlanningSettings, Profile, PromptKind, PromptTemplates,
    ResearchSettings, Rule, RuleCondition, TimeWindow, Timestamp, Version, WorktreeSetupStep,
};
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::error::database;
use crate::{Store, StoreError, change_feed};

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("invalid {what}"))
}

fn list(json: &str, what: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(json).map_err(|_| corrupt(what))
}

fn json(items: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(items).map_err(|_| corrupt("list"))
}

fn optional_list(json: Option<&str>, what: &str) -> Result<Option<Vec<String>>, StoreError> {
    json.map(|json| list(json, what)).transpose()
}

fn optional_json(items: Option<&Vec<String>>) -> Result<Option<String>, StoreError> {
    items.map(|items| json(items)).transpose()
}

fn parsed<T: std::str::FromStr>(text: &str, what: &str) -> Result<T, StoreError> {
    text.parse().map_err(|_| corrupt(what))
}

struct Names {
    by_id: HashMap<Uuid, String>,
}

impl Names {
    fn name(&self, id: Uuid) -> Result<String, StoreError> {
        self.by_id
            .get(&id)
            .cloned()
            .ok_or_else(|| corrupt("profile reference"))
    }

    fn optional(&self, id: Option<Uuid>) -> Result<Option<String>, StoreError> {
        id.map(|id| self.name(id)).transpose()
    }
}

fn id_of(ids: &HashMap<String, Uuid>, name: &str) -> Result<Uuid, StoreError> {
    ids.get(name)
        .copied()
        .ok_or_else(|| corrupt("profile reference"))
}

fn flag(value: bool) -> i64 {
    i64::from(value)
}

struct ProfileRow {
    id: Uuid,
    name: String,
    model: Option<String>,
    effort: Option<String>,
    max_turns: i64,
    timeout_minutes: i64,
}

async fn profile_rows(connection: &mut SqliteConnection) -> Result<Vec<ProfileRow>, AppError> {
    sqlx::query_as!(
        ProfileRow,
        r#"SELECT id AS "id: Uuid", name, model, effort, max_turns, timeout_minutes
           FROM dispatch_profiles ORDER BY position, name"#
    )
    .fetch_all(connection)
    .await
    .map_err(database)
}

async fn profiles(connection: &mut SqliteConnection) -> Result<(Vec<Profile>, Names), AppError> {
    let rows = profile_rows(connection).await?;
    let accounts = sqlx::query!(
        r#"SELECT link.profile_id AS "profile_id: Uuid", account.slug
           FROM dispatch_profile_accounts link JOIN claude_accounts account ON account.id = link.account_id
           ORDER BY link.profile_id, link.position"#
    )
    .fetch_all(connection)
    .await
    .map_err(database)?;
    let names = Names {
        by_id: rows.iter().map(|row| (row.id, row.name.clone())).collect(),
    };
    let profiles = rows
        .into_iter()
        .map(|row| Profile {
            id: Some(row.id),
            accounts: accounts
                .iter()
                .filter(|link| link.profile_id == row.id)
                .map(|link| link.slug.clone())
                .collect(),
            name: row.name,
            model: row.model,
            effort: row.effort,
            max_turns: row.max_turns,
            timeout_minutes: row.timeout_minutes,
        })
        .collect();
    Ok((profiles, names))
}

async fn rules(connection: &mut SqliteConnection, names: &Names) -> Result<Vec<Rule>, AppError> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id: Uuid", profile_id AS "profile_id: Uuid", when_labels, when_priorities,
                  when_files_min, when_plan_min_chars
           FROM dispatch_rules ORDER BY position"#
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(database)?;
    let accounts = sqlx::query!(
        r#"SELECT link.rule_id AS "rule_id: Uuid", account.slug
           FROM dispatch_rule_accounts link JOIN claude_accounts account ON account.id = link.account_id
           ORDER BY link.rule_id, link.position"#
    )
    .fetch_all(connection)
    .await
    .map_err(database)?;
    let mut rules = Vec::with_capacity(rows.len());
    for row in rows {
        rules.push(Rule {
            id: Some(row.id),
            when: RuleCondition {
                labels: optional_list(row.when_labels.as_deref(), "rule labels")?,
                priorities: optional_list(row.when_priorities.as_deref(), "rule priorities")?,
                files_min: row.when_files_min,
                plan_min_chars: row.when_plan_min_chars,
            },
            profile: names.name(row.profile_id)?,
            accounts: accounts
                .iter()
                .filter(|link| link.rule_id == row.id)
                .map(|link| link.slug.clone())
                .collect(),
        });
    }
    Ok(rules)
}

async fn ladder(connection: &mut SqliteConnection, names: &Names) -> Result<Vec<String>, AppError> {
    let steps = sqlx::query_scalar!(
        r#"SELECT profile_id AS "profile_id: Uuid" FROM dispatch_escalation_steps ORDER BY position"#
    )
    .fetch_all(connection)
    .await
    .map_err(database)?;
    Ok(steps
        .into_iter()
        .map(|id| names.name(id))
        .collect::<Result<_, _>>()?)
}

async fn setup_steps(
    connection: &mut SqliteConnection,
) -> Result<Vec<WorktreeSetupStep>, AppError> {
    let rows = sqlx::query!("SELECT cwd, command FROM worktree_setup_steps ORDER BY position")
        .fetch_all(connection)
        .await
        .map_err(database)?;
    Ok(rows
        .into_iter()
        .map(|row| WorktreeSetupStep {
            cwd: row.cwd,
            run: row.command,
        })
        .collect())
}

async fn templates(connection: &mut SqliteConnection) -> Result<PromptTemplates, AppError> {
    let rows = sqlx::query!("SELECT kind, template FROM prompt_templates")
        .fetch_all(connection)
        .await
        .map_err(database)?;
    let mut templates = PromptTemplates::default();
    for row in rows {
        templates.set(
            parsed::<PromptKind>(&row.kind, "prompt kind")?,
            Some(row.template),
        );
    }
    Ok(templates)
}

struct SettingsRow {
    enabled: i64,
    wip: i64,
    window_from: Option<String>,
    window_to: Option<String>,
    opt_in_label: Option<String>,
    opt_out_label: Option<String>,
    agent_prefix: String,
    stale_minutes: i64,
    max_attempts: i64,
    pause_minutes_on_limit: i64,
    prompt_extra: String,
    merge_strategy: String,
    review_before_merge: i64,
    join_worktrees: String,
    worktree_setup_timeout_min: i64,
    claude_command: String,
    claude_model: Option<String>,
    claude_effort: Option<String>,
    claude_max_turns: i64,
    claude_timeout_minutes: i64,
    claude_permission_mode: Option<String>,
    claude_allowed_tools: String,
    claude_extra_args: String,
    claude_use_subscription: i64,
    git_bash_path: Option<String>,
    default_profile_id: Uuid,
    escalate: i64,
    allow_fable: i64,
    research_auto: i64,
    research_wip: i64,
    research_max_per_day: i64,
    research_profile_id: Option<Uuid>,
    planning_auto: i64,
    planning_wip: i64,
    planning_acceptance: i64,
    planning_profile_id: Option<Uuid>,
    autopilot_enabled: i64,
    autopilot_except_label: Option<String>,
    autopilot_max_merge_sessions: i64,
    account_strategy: String,
    account_limit_pct: i64,
    account_reserve_five_hour: i64,
    account_reserve_week: i64,
    account_reset_grace_min: i64,
    config_hash: String,
    updated_at: i64,
    version: i64,
}

async fn settings_row(connection: &mut SqliteConnection) -> Result<SettingsRow, AppError> {
    sqlx::query_as!(
        SettingsRow,
        r#"SELECT enabled, wip, window_from, window_to, opt_in_label, opt_out_label, agent_prefix,
                  stale_minutes, max_attempts, pause_minutes_on_limit, prompt_extra, merge_strategy,
                  review_before_merge, join_worktrees, worktree_setup_timeout_min,
                  claude_command, claude_model, claude_effort, claude_max_turns,
                  claude_timeout_minutes, claude_permission_mode, claude_allowed_tools,
                  claude_extra_args, claude_use_subscription, git_bash_path,
                  default_profile_id AS "default_profile_id: Uuid", escalate, allow_fable,
                  research_auto, research_wip, research_max_per_day,
                  research_profile_id AS "research_profile_id: Uuid",
                  planning_auto, planning_wip, planning_acceptance,
                  planning_profile_id AS "planning_profile_id: Uuid",
                  autopilot_enabled, autopilot_except_label, autopilot_max_merge_sessions,
                  account_strategy, account_limit_pct, account_reserve_five_hour,
                  account_reserve_week, account_reset_grace_min, config_hash, updated_at, version
           FROM dispatcher_settings WHERE id = 1"#
    )
    .fetch_one(connection)
    .await
    .map_err(database)
}

impl SettingsRow {
    fn claude(&self) -> Result<ClaudeSettings, StoreError> {
        Ok(ClaudeSettings {
            command: self.claude_command.clone(),
            model: self.claude_model.clone(),
            effort: self.claude_effort.clone(),
            max_turns: self.claude_max_turns,
            timeout_minutes: self.claude_timeout_minutes,
            permission_mode: self.claude_permission_mode.clone(),
            allowed_tools: list(&self.claude_allowed_tools, "allowed tools")?,
            use_subscription: self.claude_use_subscription != 0,
            extra_args: list(&self.claude_extra_args, "extra arguments")?,
            git_bash_path: self.git_bash_path.clone(),
        })
    }

    fn window(&self) -> Option<TimeWindow> {
        match (&self.window_from, &self.window_to) {
            (Some(from), Some(to)) => Some(TimeWindow {
                from: from.clone(),
                to: to.clone(),
            }),
            _ => None,
        }
    }
}

struct Lists {
    profiles: Vec<Profile>,
    rules: Vec<Rule>,
    ladder: Vec<String>,
    worktree_setup: Vec<WorktreeSetupStep>,
    prompts: PromptTemplates,
}

fn assemble(row: SettingsRow, lists: Lists, names: &Names) -> Result<StoredConfig, StoreError> {
    let config = DispatcherConfig {
        enabled: row.enabled != 0,
        wip: row.wip,
        window: row.window(),
        opt_in_label: row.opt_in_label.clone(),
        opt_out_label: row.opt_out_label.clone(),
        agent_prefix: row.agent_prefix.clone(),
        stale_minutes: row.stale_minutes,
        max_attempts: row.max_attempts,
        pause_minutes_on_limit: row.pause_minutes_on_limit,
        prompt_extra: row.prompt_extra.clone(),
        prompts: lists.prompts,
        merge_strategy: parsed::<MergeStrategy>(&row.merge_strategy, "merge strategy")?,
        review_before_merge: row.review_before_merge != 0,
        join_worktrees: parsed::<JoinWorktrees>(&row.join_worktrees, "worktree joining")?,
        worktree_setup: lists.worktree_setup,
        worktree_setup_timeout_minutes: row.worktree_setup_timeout_min,
        claude: row.claude()?,
        profiles: lists.profiles,
        default_profile: names.name(row.default_profile_id)?,
        rules: lists.rules,
        escalate: row.escalate != 0,
        ladder: lists.ladder,
        allow_fable: row.allow_fable != 0,
        research: ResearchSettings {
            auto: row.research_auto != 0,
            wip: row.research_wip,
            max_per_day: row.research_max_per_day,
            profile: names.optional(row.research_profile_id)?,
        },
        planning: PlanningSettings {
            auto: row.planning_auto != 0,
            wip: row.planning_wip,
            acceptance: row.planning_acceptance != 0,
            profile: names.optional(row.planning_profile_id)?,
        },
        autopilot: AutopilotSettings {
            enabled: row.autopilot_enabled != 0,
            except_label: row.autopilot_except_label.clone(),
            max_merge_sessions: row.autopilot_max_merge_sessions,
        },
        account_strategy: parsed::<AccountStrategy>(&row.account_strategy, "account strategy")?,
        account_limit_percent: row.account_limit_pct,
        account_reserve_five_hour: row.account_reserve_five_hour,
        account_reserve_week: row.account_reserve_week,
        account_reset_grace_minutes: row.account_reset_grace_min,
    };
    Ok(StoredConfig {
        config,
        hash: row.config_hash,
        version: Version::from_stored(row.version),
        updated_at: Timestamp::from_millis(row.updated_at),
    })
}

async fn read(connection: &mut SqliteConnection) -> Result<StoredConfig, AppError> {
    let (profiles, names) = profiles(connection).await?;
    let lists = Lists {
        rules: rules(connection, &names).await?,
        ladder: ladder(connection, &names).await?,
        worktree_setup: setup_steps(connection).await?,
        prompts: templates(connection).await?,
        profiles,
    };
    let row = settings_row(connection).await?;
    Ok(assemble(row, lists, &names)?)
}

/// Why a save stopped; only `App` is a failure, the others are for the caller to name.
enum Stop {
    Stale,
    UnknownAccount(FieldProblem),
    App(AppError),
}

impl From<AppError> for Stop {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<StoreError> for Stop {
    fn from(error: StoreError) -> Self {
        Self::App(error.into())
    }
}

fn unknown_account(field: &str, slug: &str) -> Stop {
    Stop::UnknownAccount(FieldProblem {
        field: field.to_owned(),
        message: format!("there is no account with the id \"{slug}\""),
    })
}

fn profile_changed(old: &Profile, new: &Profile) -> bool {
    old.name != new.name
        || old.model != new.model
        || old.effort != new.effort
        || old.max_turns != new.max_turns
        || old.timeout_minutes != new.timeout_minutes
        || old.accounts != new.accounts
}

/// The stored profile each draft profile stands for: by id, else by name.
fn match_profiles(existing: &[Profile], draft: &[Profile]) -> Vec<Option<usize>> {
    let mut matched: Vec<Option<usize>> = draft
        .iter()
        .map(|profile| {
            profile
                .id
                .and_then(|id| existing.iter().position(|found| found.id == Some(id)))
        })
        .collect();
    let mut claimed: Vec<usize> = matched.iter().flatten().copied().collect();
    for (slot, profile) in matched.iter_mut().zip(draft) {
        if slot.is_some() {
            continue;
        }
        let by_name = existing
            .iter()
            .position(|found| found.name == profile.name)
            .filter(|index| !claimed.contains(index));
        if let Some(index) = by_name {
            claimed.push(index);
            *slot = Some(index);
        }
    }
    matched
}

/// Brings the profile rows to the configuration's profiles and returns their ids by name.
async fn write_profiles(
    connection: &mut SqliteConnection,
    config: &DispatcherConfig,
    ids: &dyn IdGenerator,
) -> Result<HashMap<String, Uuid>, AppError> {
    let (existing, _) = profiles(connection).await?;
    let matched = match_profiles(&existing, &config.profiles);
    // names are unique: park rows that change their name so renames and swaps cannot collide
    for (at, row) in existing.iter().enumerate() {
        let target = matched.iter().position(|slot| *slot == Some(at));
        if target.is_some_and(|draft| config.profiles[draft].name == row.name) {
            continue;
        }
        let id = row.id.ok_or_else(|| corrupt("profile id"))?;
        sqlx::query!(
            "UPDATE dispatch_profiles SET name = lower(hex(id)) WHERE id = ?",
            id
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    }
    let mut by_name = HashMap::new();
    for (at, profile) in config.profiles.iter().enumerate() {
        let position = i64::try_from(at).map_err(|_| corrupt("position"))?;
        let found = matched[at].map(|index| &existing[index]);
        let id = match found {
            Some(row) => row.id.ok_or_else(|| corrupt("profile id"))?,
            None => ids.next_uuid(),
        };
        if let Some(row) = found {
            let bump = i64::from(profile_changed(row, profile));
            sqlx::query!(
                "UPDATE dispatch_profiles
                 SET name = ?, model = ?, effort = ?, max_turns = ?, timeout_minutes = ?,
                     position = ?, version = version + ?
                 WHERE id = ?",
                profile.name,
                profile.model,
                profile.effort,
                profile.max_turns,
                profile.timeout_minutes,
                position,
                bump,
                id,
            )
            .execute(&mut *connection)
            .await
            .map_err(database)?;
        } else {
            sqlx::query!(
                "INSERT INTO dispatch_profiles
                   (id, name, model, effort, max_turns, timeout_minutes, position)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                id,
                profile.name,
                profile.model,
                profile.effort,
                profile.max_turns,
                profile.timeout_minutes,
                position,
            )
            .execute(&mut *connection)
            .await
            .map_err(database)?;
        }
        by_name.insert(profile.name.clone(), id);
    }
    Ok(by_name)
}

async fn remove_other_profiles(
    connection: &mut SqliteConnection,
    keep: &HashMap<String, Uuid>,
) -> Result<(), AppError> {
    for row in profile_rows(connection).await? {
        if keep.values().any(|id| *id == row.id) {
            continue;
        }
        sqlx::query!("DELETE FROM dispatch_profiles WHERE id = ?", row.id)
            .execute(&mut *connection)
            .await
            .map_err(database)?;
    }
    Ok(())
}

async fn account_ids(connection: &mut SqliteConnection) -> Result<HashMap<String, Uuid>, AppError> {
    let rows = sqlx::query!(r#"SELECT id AS "id: Uuid", slug FROM claude_accounts"#)
        .fetch_all(connection)
        .await
        .map_err(database)?;
    Ok(rows.into_iter().map(|row| (row.slug, row.id)).collect())
}

async fn write_profile_accounts(
    connection: &mut SqliteConnection,
    config: &DispatcherConfig,
    profiles: &HashMap<String, Uuid>,
    accounts: &HashMap<String, Uuid>,
) -> Result<(), Stop> {
    sqlx::query!("DELETE FROM dispatch_profile_accounts")
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    for (at, profile) in config.profiles.iter().enumerate() {
        let profile_id = id_of(profiles, &profile.name)?;
        for (position, slug) in profile.accounts.iter().enumerate() {
            let field = format!("profiles[{at}].accounts[{position}]");
            let account_id = *accounts
                .get(slug)
                .ok_or_else(|| unknown_account(&field, slug))?;
            let position = i64::try_from(position).map_err(|_| corrupt("position"))?;
            sqlx::query!(
                "INSERT INTO dispatch_profile_accounts (profile_id, account_id, position)
                 VALUES (?, ?, ?)",
                profile_id,
                account_id,
                position,
            )
            .execute(&mut *connection)
            .await
            .map_err(database)?;
        }
    }
    Ok(())
}

fn rule_changed(old: &Rule, new: &Rule) -> bool {
    old.when != new.when || old.profile != new.profile || old.accounts != new.accounts
}

/// The stored rule each draft rule stands for: by id, else the one at its position.
fn match_rules(existing: &[Rule], draft: &[Rule]) -> Vec<Option<usize>> {
    let mut matched: Vec<Option<usize>> = draft
        .iter()
        .map(|rule| {
            rule.id
                .and_then(|id| existing.iter().position(|found| found.id == Some(id)))
        })
        .collect();
    let mut claimed: Vec<usize> = matched.iter().flatten().copied().collect();
    for (at, slot) in matched.iter_mut().enumerate() {
        if slot.is_none() && at < existing.len() && !claimed.contains(&at) {
            *slot = Some(at);
            claimed.push(at);
        }
    }
    matched
}

async fn write_rules(
    connection: &mut SqliteConnection,
    config: &DispatcherConfig,
    profiles_by_name: &HashMap<String, Uuid>,
    accounts: &HashMap<String, Uuid>,
    ids: &dyn IdGenerator,
) -> Result<(), Stop> {
    let (_, names) = profiles(connection).await?;
    let existing = rules(connection, &names).await?;
    let matched = match_rules(&existing, &config.rules);
    sqlx::query!("DELETE FROM dispatch_rule_accounts")
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    for (at, rule) in existing.iter().enumerate() {
        if matched.contains(&Some(at)) {
            continue;
        }
        let id = rule.id.ok_or_else(|| corrupt("rule id"))?;
        sqlx::query!("DELETE FROM dispatch_rules WHERE id = ?", id)
            .execute(&mut *connection)
            .await
            .map_err(database)?;
    }
    sqlx::query!("UPDATE dispatch_rules SET position = position + 1000000")
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    for (at, rule) in config.rules.iter().enumerate() {
        let position = i64::try_from(at + 1).map_err(|_| corrupt("position"))?;
        let found = matched[at].map(|index| &existing[index]);
        let id = match found {
            Some(row) => row.id.ok_or_else(|| corrupt("rule id"))?,
            None => ids.next_uuid(),
        };
        let profile_id = id_of(profiles_by_name, &rule.profile)?;
        let labels = optional_json(rule.when.labels.as_ref())?;
        let priorities = optional_json(rule.when.priorities.as_ref())?;
        if let Some(row) = found {
            let bump = i64::from(rule_changed(row, rule));
            sqlx::query!(
                "UPDATE dispatch_rules
                 SET position = ?, profile_id = ?, when_labels = ?, when_priorities = ?,
                     when_files_min = ?, when_plan_min_chars = ?, version = version + ?
                 WHERE id = ?",
                position,
                profile_id,
                labels,
                priorities,
                rule.when.files_min,
                rule.when.plan_min_chars,
                bump,
                id,
            )
            .execute(&mut *connection)
            .await
            .map_err(database)?;
        } else {
            sqlx::query!(
                "INSERT INTO dispatch_rules
                   (id, position, profile_id, when_labels, when_priorities, when_files_min,
                    when_plan_min_chars)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                id,
                position,
                profile_id,
                labels,
                priorities,
                rule.when.files_min,
                rule.when.plan_min_chars,
            )
            .execute(&mut *connection)
            .await
            .map_err(database)?;
        }
        for (index, slug) in rule.accounts.iter().enumerate() {
            let field = format!("rules[{at}].accounts[{index}]");
            let account_id = *accounts
                .get(slug)
                .ok_or_else(|| unknown_account(&field, slug))?;
            let index = i64::try_from(index).map_err(|_| corrupt("position"))?;
            sqlx::query!(
                "INSERT INTO dispatch_rule_accounts (rule_id, account_id, position)
                 VALUES (?, ?, ?)",
                id,
                account_id,
                index,
            )
            .execute(&mut *connection)
            .await
            .map_err(database)?;
        }
    }
    Ok(())
}

async fn write_lists(
    connection: &mut SqliteConnection,
    config: &DispatcherConfig,
    profiles: &HashMap<String, Uuid>,
    at: Timestamp,
) -> Result<(), AppError> {
    sqlx::query!("DELETE FROM dispatch_escalation_steps")
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    for (position, name) in config.ladder.iter().enumerate() {
        let position = i64::try_from(position).map_err(|_| corrupt("position"))?;
        let profile_id = id_of(profiles, name)?;
        sqlx::query!(
            "INSERT INTO dispatch_escalation_steps (position, profile_id) VALUES (?, ?)",
            position,
            profile_id,
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    }
    sqlx::query!("DELETE FROM worktree_setup_steps")
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    for (position, step) in config.worktree_setup.iter().enumerate() {
        let position = i64::try_from(position).map_err(|_| corrupt("position"))?;
        sqlx::query!(
            "INSERT INTO worktree_setup_steps (position, cwd, command) VALUES (?, ?, ?)",
            position,
            step.cwd,
            step.run,
        )
        .execute(&mut *connection)
        .await
        .map_err(database)?;
    }
    write_templates(connection, config, at).await
}

async fn write_templates(
    connection: &mut SqliteConnection,
    config: &DispatcherConfig,
    at: Timestamp,
) -> Result<(), AppError> {
    let millis = at.as_millis();
    for kind in [
        PromptKind::Work,
        PromptKind::Plan,
        PromptKind::Research,
        PromptKind::Merge,
    ] {
        let name = kind.as_str();
        match config.prompts.get(kind) {
            Some(template) => {
                sqlx::query!(
                    "INSERT INTO prompt_templates (kind, template, updated_at) VALUES (?, ?, ?)
                     ON CONFLICT (kind) DO UPDATE
                       SET template = excluded.template, updated_at = excluded.updated_at,
                           version = version + 1
                       WHERE template <> excluded.template",
                    name,
                    template,
                    millis,
                )
                .execute(&mut *connection)
                .await
                .map_err(database)?;
            }
            None => {
                sqlx::query!("DELETE FROM prompt_templates WHERE kind = ?", name)
                    .execute(&mut *connection)
                    .await
                    .map_err(database)?;
            }
        }
    }
    Ok(())
}

async fn write_settings(
    connection: &mut SqliteConnection,
    save: &ConfigSave<'_>,
    profiles: &HashMap<String, Uuid>,
) -> Result<(), Stop> {
    let config = save.config;
    let claude = &config.claude;
    let (window_from, window_to) = config.window.as_ref().map_or((None, None), |window| {
        (Some(window.from.as_str()), Some(window.to.as_str()))
    });
    let allowed_tools = json(&claude.allowed_tools)?;
    let extra_args = json(&claude.extra_args)?;
    let profile_id = |name: &str| profiles.get(name).copied();
    let default_profile = profile_id(&config.default_profile);
    let research_profile = config.research.profile.as_deref().and_then(profile_id);
    let planning_profile = config.planning.profile.as_deref().and_then(profile_id);
    let updated = sqlx::query!(
        "UPDATE dispatcher_settings SET
           enabled = ?, wip = ?, window_from = ?, window_to = ?, opt_in_label = ?,
           opt_out_label = ?, agent_prefix = ?, stale_minutes = ?, max_attempts = ?,
           pause_minutes_on_limit = ?, prompt_extra = ?, merge_strategy = ?,
           review_before_merge = ?, join_worktrees = ?, worktree_setup_timeout_min = ?,
           claude_command = ?, claude_model = ?, claude_effort = ?, claude_max_turns = ?,
           claude_timeout_minutes = ?, claude_permission_mode = ?, claude_allowed_tools = ?,
           claude_extra_args = ?, claude_use_subscription = ?, git_bash_path = ?,
           default_profile_id = ?, escalate = ?, allow_fable = ?, research_auto = ?,
           research_wip = ?, research_max_per_day = ?, research_profile_id = ?,
           planning_auto = ?, planning_wip = ?, planning_acceptance = ?, planning_profile_id = ?,
           autopilot_enabled = ?, autopilot_except_label = ?, autopilot_max_merge_sessions = ?,
           account_strategy = ?, account_limit_pct = ?, account_reserve_five_hour = ?,
           account_reserve_week = ?, account_reset_grace_min = ?, config_hash = ?,
           updated_at = ?, version = version + 1
         WHERE id = 1 AND version = ?",
        flag(config.enabled),
        config.wip,
        window_from,
        window_to,
        config.opt_in_label,
        config.opt_out_label,
        config.agent_prefix,
        config.stale_minutes,
        config.max_attempts,
        config.pause_minutes_on_limit,
        config.prompt_extra,
        config.merge_strategy.as_str(),
        flag(config.review_before_merge),
        config.join_worktrees.as_str(),
        config.worktree_setup_timeout_minutes,
        claude.command,
        claude.model,
        claude.effort,
        claude.max_turns,
        claude.timeout_minutes,
        claude.permission_mode,
        allowed_tools,
        extra_args,
        flag(claude.use_subscription),
        claude.git_bash_path,
        default_profile,
        flag(config.escalate),
        flag(config.allow_fable),
        flag(config.research.auto),
        config.research.wip,
        config.research.max_per_day,
        research_profile,
        flag(config.planning.auto),
        config.planning.wip,
        flag(config.planning.acceptance),
        planning_profile,
        flag(config.autopilot.enabled),
        config.autopilot.except_label,
        config.autopilot.max_merge_sessions,
        config.account_strategy.as_str(),
        config.account_limit_percent,
        config.account_reserve_five_hour,
        config.account_reserve_week,
        config.account_reset_grace_minutes,
        save.fingerprint.hash,
        save.at.as_millis(),
        save.expected.value(),
    )
    .execute(connection)
    .await
    .map_err(database)?;
    if updated.rows_affected() == 0 {
        return Err(Stop::Stale);
    }
    Ok(())
}

async fn keep_version(
    connection: &mut SqliteConnection,
    fingerprint: &Fingerprint,
    at: Timestamp,
) -> Result<(), AppError> {
    let millis = at.as_millis();
    sqlx::query!(
        "INSERT OR IGNORE INTO dispatch_config_versions (hash, content, first_seen_at)
         VALUES (?, ?, ?)",
        fingerprint.hash,
        fingerprint.content,
        millis,
    )
    .execute(connection)
    .await
    .map_err(database)?;
    Ok(())
}

async fn write_all(
    connection: &mut SqliteConnection,
    save: &ConfigSave<'_>,
) -> Result<Change, Stop> {
    let accounts = account_ids(connection).await?;
    let profiles = write_profiles(connection, save.config, save.ids).await?;
    write_profile_accounts(connection, save.config, &profiles, &accounts).await?;
    write_rules(connection, save.config, &profiles, &accounts, save.ids).await?;
    write_lists(connection, save.config, &profiles, save.at).await?;
    write_settings(connection, save, &profiles).await?;
    remove_other_profiles(connection, &profiles).await?;
    keep_version(connection, save.fingerprint, save.at).await?;
    Ok(change_feed::record(
        connection,
        Entity::DispatcherConfig,
        CONFIG_ENTITY_ID,
        ChangeOp::Update,
        save.at,
    )
    .await?)
}

/// After accounts left the configuration: new hash and version, and the change.
pub(crate) async fn resync_after_account_removed(
    connection: &mut SqliteConnection,
    at: Timestamp,
) -> Result<Change, AppError> {
    let fingerprint = read(connection).await?.config.fingerprint();
    let millis = at.as_millis();
    sqlx::query!(
        "UPDATE dispatcher_settings SET config_hash = ?, updated_at = ?, version = version + 1
         WHERE id = 1",
        fingerprint.hash,
        millis,
    )
    .execute(&mut *connection)
    .await
    .map_err(database)?;
    keep_version(connection, &fingerprint, at).await?;
    change_feed::record(
        connection,
        Entity::DispatcherConfig,
        CONFIG_ENTITY_ID,
        ChangeOp::Update,
        at,
    )
    .await
    .map_err(AppError::from)
}

#[async_trait]
impl DispatcherConfigRepository for Store {
    async fn load(&self) -> Result<StoredConfig, AppError> {
        let mut tx = self.reader.begin().await.map_err(database)?;
        let stored = read(&mut tx).await?;
        tx.commit().await.map_err(database)?;
        Ok(stored)
    }

    async fn save(&self, save: &ConfigSave<'_>) -> Result<SaveOutcome, AppError> {
        let mut tx = self.begin().await?;
        match write_all(&mut tx, save).await {
            Ok(change) => {
                tx.commit().await.map_err(database)?;
                Ok(SaveOutcome::Saved(change))
            }
            Err(Stop::Stale) => Ok(SaveOutcome::Stale),
            Err(Stop::UnknownAccount(problem)) => Ok(SaveOutcome::UnknownAccount(problem)),
            Err(Stop::App(error)) => Err(error),
        }
    }

    async fn record_hash(
        &self,
        fingerprint: &Fingerprint,
        expected: Version,
        at: Timestamp,
    ) -> Result<(), AppError> {
        let mut tx = self.begin().await?;
        let version = expected.value();
        let updated = sqlx::query!(
            "UPDATE dispatcher_settings SET config_hash = ? WHERE id = 1 AND version = ?",
            fingerprint.hash,
            version,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if updated.rows_affected() > 0 {
            keep_version(&mut tx, fingerprint, at).await?;
            tx.commit().await.map_err(database)?;
        }
        Ok(())
    }

    async fn versions(&self) -> Result<Vec<ConfigVersion>, AppError> {
        let rows = sqlx::query!(
            "SELECT hash, content, first_seen_at FROM dispatch_config_versions
             ORDER BY first_seen_at, hash"
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows
            .into_iter()
            .map(|row| ConfigVersion {
                hash: row.hash,
                content: row.content,
                first_seen_at: Timestamp::from_millis(row.first_seen_at),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn refused(sql: &str) -> bool {
        let store = Store::in_memory().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&store.writer)
            .await
            .is_err()
    }

    #[tokio::test]
    async fn the_seeded_rows_equal_the_default_configuration() {
        let store = Store::in_memory().await.unwrap();
        let stored = store.load().await.unwrap();
        assert_eq!(stored.config.without_ids(), DispatcherConfig::default());
        assert_eq!(stored.hash, "");
        let actor: (String,) = sqlx::query_as(
            "SELECT a.kind FROM actors a JOIN dispatcher_settings s ON s.dispatcher_actor_id = a.id",
        )
        .fetch_one(&store.writer)
        .await
        .unwrap();
        assert_eq!(actor.0, "dispatcher");
    }

    #[tokio::test]
    async fn the_schema_guards_the_settings_row() {
        assert!(refused("UPDATE dispatcher_settings SET id = 2").await);
        assert!(refused("UPDATE dispatcher_settings SET window_from = '08:00'").await);
        assert!(
            refused("UPDATE dispatcher_settings SET window_from = '8:00', window_to = '9:00'")
                .await
        );
        assert!(refused("UPDATE dispatcher_settings SET research_wip = 6").await);
        assert!(refused("UPDATE dispatcher_settings SET planning_wip = 0").await);
        assert!(refused("UPDATE dispatcher_settings SET autopilot_max_merge_sessions = 6").await);
        assert!(refused("UPDATE dispatcher_settings SET account_limit_pct = 49").await);
        assert!(refused("UPDATE dispatcher_settings SET join_worktrees = 'sameSpec'").await);
        assert!(refused("UPDATE dispatcher_settings SET merge_strategy = 'rebase'").await);
        assert!(refused("UPDATE dispatcher_settings SET claude_allowed_tools = 'not json'").await);
        assert!(refused("UPDATE dispatcher_settings SET default_profile_id = x'01'").await);
        assert!(
            !refused("UPDATE dispatcher_settings SET window_from = '18:00', window_to = '07:00'")
                .await
        );
    }

    #[tokio::test]
    async fn the_schema_guards_profiles_rules_and_templates() {
        assert!(refused("UPDATE dispatch_profiles SET max_turns = 0").await);
        assert!(refused("UPDATE dispatch_profiles SET name = 'small' WHERE name = 'large'").await);
        assert!(refused("DELETE FROM dispatch_profiles WHERE name = 'medium'").await);
        assert!(refused("UPDATE dispatch_rules SET when_labels = NULL WHERE position = 1").await);
        assert!(refused("UPDATE dispatch_rules SET position = 1 WHERE position = 2").await);
        assert!(refused("UPDATE dispatch_rules SET when_files_min = 0 WHERE position = 4").await);
        assert!(
            refused(
                "INSERT INTO prompt_templates (kind, template, updated_at) VALUES ('chat', 'x', 1)"
            )
            .await
        );
        assert!(
            refused(
                "INSERT INTO prompt_templates (kind, template, updated_at) VALUES ('work', '  ', 1)"
            )
            .await
        );
        assert!(
            refused(
                "INSERT INTO worktree_setup_steps (position, cwd, command) VALUES (0, '.', '')"
            )
            .await
        );
        assert!(refused("INSERT INTO dispatch_config_versions (hash, content, first_seen_at) VALUES ('abc', '{}', 1)").await);
        assert!(
            !refused(
                "INSERT INTO prompt_templates (kind, template, updated_at) VALUES ('work', 'x', 1)"
            )
            .await
        );
    }

    #[tokio::test]
    async fn kept_profiles_and_rules_keep_their_ids_across_updates() {
        let store = Store::in_memory().await.unwrap();
        let ids = nullradix_app::UuidV7Generator;
        let ids_before: Vec<(String, Uuid)> =
            sqlx::query_as("SELECT name, id FROM dispatch_profiles ORDER BY name")
                .fetch_all(&store.writer)
                .await
                .unwrap();
        let rules_before: Vec<(Uuid,)> =
            sqlx::query_as("SELECT id FROM dispatch_rules ORDER BY position")
                .fetch_all(&store.writer)
                .await
                .unwrap();
        let mut config = DispatcherConfig::default();
        config.profiles[1].max_turns = 99;
        config.rules.pop();
        let fingerprint = config.fingerprint();
        let save = ConfigSave {
            config: &config,
            fingerprint: &fingerprint,
            expected: Version::INITIAL,
            at: Timestamp::from_millis(5),
            ids: &ids,
        };
        store.save(&save).await.unwrap();
        let after: Vec<(String, Uuid, i64)> =
            sqlx::query_as("SELECT name, id, version FROM dispatch_profiles ORDER BY name")
                .fetch_all(&store.writer)
                .await
                .unwrap();
        for ((name, id), (kept_name, kept_id, version)) in ids_before.iter().zip(&after) {
            assert_eq!((name, id), (kept_name, kept_id));
            assert_eq!(*version, i64::from(name == "medium") + 1);
        }
        let rules_after: Vec<(Uuid,)> =
            sqlx::query_as("SELECT id FROM dispatch_rules ORDER BY position")
                .fetch_all(&store.writer)
                .await
                .unwrap();
        assert_eq!(rules_after, rules_before[..3]);
    }

    #[tokio::test]
    async fn reordering_bumps_no_row_version_and_a_rename_bumps_only_the_profile() {
        let store = Store::in_memory().await.unwrap();
        let ids = nullradix_app::UuidV7Generator;
        let mut config = store.load().await.unwrap().config;
        config.profiles.reverse();
        config.rules.reverse();
        let mut version = Version::INITIAL;
        for rename in [false, true] {
            if rename {
                config.profiles[0].name = String::from("huge");
                config.ladder[2] = String::from("huge");
                config.rules[0].profile = String::from("huge");
                config.rules[1].profile = String::from("huge");
                config.rules[2].profile = String::from("huge");
            }
            let fingerprint = config.fingerprint();
            let save = ConfigSave {
                config: &config,
                fingerprint: &fingerprint,
                expected: version,
                at: Timestamp::from_millis(5),
                ids: &ids,
            };
            assert!(matches!(
                store.save(&save).await.unwrap(),
                SaveOutcome::Saved(_)
            ));
            version = version.next();
            let profiles: Vec<(String, i64)> =
                sqlx::query_as("SELECT name, version FROM dispatch_profiles ORDER BY position")
                    .fetch_all(&store.writer)
                    .await
                    .unwrap();
            let bumped = profiles.iter().filter(|(_, version)| *version == 2).count();
            assert_eq!(bumped, usize::from(rename), "{profiles:?}");
            let rules: Vec<(i64,)> = sqlx::query_as("SELECT version FROM dispatch_rules")
                .fetch_all(&store.writer)
                .await
                .unwrap();
            assert!(rules.iter().all(|(version,)| *version == 1), "{rules:?}");
        }
    }
}
