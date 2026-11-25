use axum::Json;
use axum::extract::State;
use nullradix_app::AppError;
use nullradix_app::dispatcher_config::ConfigView;
use nullradix_domain::{
    AccountStrategy, AutopilotSettings, ClaudeSettings, DispatcherConfig, JoinWorktrees,
    MergeStrategy, PlanningSettings, Profile, PromptTemplates, ResearchSettings, Rule,
    RuleCondition, TimeWindow, WorktreeSetupStep,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{ApiError, ApiJson, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MergeStrategyDto {
    Merge,
    Squash,
}

impl From<MergeStrategy> for MergeStrategyDto {
    fn from(value: MergeStrategy) -> Self {
        match value {
            MergeStrategy::Merge => Self::Merge,
            MergeStrategy::Squash => Self::Squash,
        }
    }
}

impl From<MergeStrategyDto> for MergeStrategy {
    fn from(value: MergeStrategyDto) -> Self {
        match value {
            MergeStrategyDto::Merge => Self::Merge,
            MergeStrategyDto::Squash => Self::Squash,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum JoinWorktreesDto {
    None,
    SameSpec,
}

impl From<JoinWorktrees> for JoinWorktreesDto {
    fn from(value: JoinWorktrees) -> Self {
        match value {
            JoinWorktrees::None => Self::None,
            JoinWorktrees::SameSpec => Self::SameSpec,
        }
    }
}

impl From<JoinWorktreesDto> for JoinWorktrees {
    fn from(value: JoinWorktreesDto) -> Self {
        match value {
            JoinWorktreesDto::None => Self::None,
            JoinWorktreesDto::SameSpec => Self::SameSpec,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AccountStrategyDto {
    MostFree,
    Order,
}

impl From<AccountStrategy> for AccountStrategyDto {
    fn from(value: AccountStrategy) -> Self {
        match value {
            AccountStrategy::MostFree => Self::MostFree,
            AccountStrategy::Order => Self::Order,
        }
    }
}

impl From<AccountStrategyDto> for AccountStrategy {
    fn from(value: AccountStrategyDto) -> Self {
        match value {
            AccountStrategyDto::MostFree => Self::MostFree,
            AccountStrategyDto::Order => Self::Order,
        }
    }
}

/// Local time window in which new runs may start; `to` before `from` wraps past midnight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeWindowDto {
    /// `HH:MM`, inclusive.
    pub from: String,
    /// `HH:MM`, exclusive.
    pub to: String,
}

/// Own prompt templates per kind of run; none means the built-in template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptTemplatesDto {
    pub work: Option<String>,
    pub plan: Option<String>,
    pub research: Option<String>,
    pub merge: Option<String>,
}

/// Command run in a fresh worktree before Claude starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorktreeSetupStepDto {
    /// Folder relative to the worktree root; `.` is the root itself.
    pub cwd: String,
    pub run: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeSettingsDto {
    /// `claude` or a full path.
    pub command: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Default turns for a new profile and part of the stale threshold.
    pub max_turns: i64,
    /// Default timeout for a new profile and part of the stale threshold.
    pub timeout_minutes: i64,
    pub permission_mode: Option<String>,
    /// What the agent may do without asking.
    pub allowed_tools: Vec<String>,
    /// Remove API key variables from the environment of runs.
    pub use_subscription: bool,
    pub extra_args: Vec<String>,
    /// Path of `bash.exe` of Git for Windows; none looks for it.
    pub git_bash_path: Option<String>,
}

/// Model, effort and limits of a run; the order of profiles is the escalation order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileDto {
    /// Stable id of a stored profile; leave it out for a new one.
    pub id: Option<Uuid>,
    pub name: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub max_turns: i64,
    pub timeout_minutes: i64,
    /// Account ids the profile may use in this order; none means any enabled account.
    pub accounts: Vec<String>,
}

/// Conditions of a rule; all that are set must hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleConditionDto {
    /// The ticket carries one of these labels.
    pub labels: Option<Vec<String>>,
    /// The priority is one of these.
    pub priorities: Option<Vec<String>>,
    /// At least this many rows in the plan table under `## Integration`.
    pub files_min: Option<i64>,
    /// The plan has at least this many characters.
    pub plan_min_chars: Option<i64>,
}

/// When `when` holds, `profile` applies; the first matching rule wins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleDto {
    /// Stable id of a stored rule; leave it out for a new one.
    pub id: Option<Uuid>,
    pub when: RuleConditionDto,
    /// Name of the profile.
    pub profile: String,
    /// Account ids that replace those of the profile; none means as the profile.
    pub accounts: Vec<String>,
}

/// Research runs on open questions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchSettingsDto {
    pub auto: bool,
    /// Parallel research runs from 1 to 5.
    pub wip: i64,
    pub max_per_day: i64,
    /// Profile name; none uses `research` if it exists, else the default.
    pub profile: Option<String>,
}

/// Planning runs on tickets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanningSettingsDto {
    pub auto: bool,
    /// Parallel planning runs from 1 to 5.
    pub wip: i64,
    /// The plan also fills the acceptance of the ticket.
    pub acceptance: bool,
    /// Profile name; none uses `planning` if it exists, else the default.
    pub profile: Option<String>,
}

/// No approvals, research answers questions, Claude resolves merge conflicts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutopilotSettingsDto {
    pub enabled: bool,
    /// Tickets with this label run without the autopilot.
    pub except_label: Option<String>,
    /// Merge sessions per conflict from 1 to 5.
    pub max_merge_sessions: i64,
}

/// The dispatcher's whole configuration; a save replaces all of it.
#[allow(
    clippy::struct_excessive_bools,
    reason = "one switch per stored setting"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DispatcherConfigDto {
    /// Off means nothing new starts; running runs are still evaluated.
    pub enabled: bool,
    /// Tickets worked on at the same time.
    pub wip: i64,
    pub window: Option<TimeWindowDto>,
    /// Only tickets with this label are picked up; none means all ready tickets.
    pub opt_in_label: Option<String>,
    /// Tickets with this label are never picked up.
    pub opt_out_label: Option<String>,
    pub agent_prefix: String,
    /// Minutes without news before a run counts as orphaned; raised above the longest timeout.
    pub stale_minutes: i64,
    pub max_attempts: i64,
    pub pause_minutes_on_limit: i64,
    pub prompt_extra: String,
    pub prompts: PromptTemplatesDto,
    pub merge_strategy: MergeStrategyDto,
    pub review_before_merge: bool,
    pub join_worktrees: JoinWorktreesDto,
    pub worktree_setup: Vec<WorktreeSetupStepDto>,
    pub worktree_setup_timeout_minutes: i64,
    pub claude: ClaudeSettingsDto,
    /// In escalation order, cheapest first.
    pub profiles: Vec<ProfileDto>,
    pub default_profile: String,
    pub rules: Vec<RuleDto>,
    pub escalate: bool,
    /// Profile names from cheap to expensive.
    pub ladder: Vec<String>,
    pub allow_fable: bool,
    pub research: ResearchSettingsDto,
    pub planning: PlanningSettingsDto,
    pub autopilot: AutopilotSettingsDto,
    pub account_strategy: AccountStrategyDto,
    pub account_limit_percent: i64,
    pub account_reserve_five_hour: i64,
    pub account_reserve_week: i64,
    pub account_reset_grace_minutes: i64,
}

/// The configuration with the version a save has to be based on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherConfigView {
    pub config: DispatcherConfigDto,
    /// Short hash naming this configuration for the runs started with it.
    pub hash: String,
    pub updated_at: i64,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateDispatcherConfig {
    /// Version the change is based on; a newer stored version fails with `version-conflict`.
    pub version: i64,
    pub config: DispatcherConfigDto,
}

impl From<TimeWindow> for TimeWindowDto {
    fn from(window: TimeWindow) -> Self {
        Self {
            from: window.from,
            to: window.to,
        }
    }
}

impl From<TimeWindowDto> for TimeWindow {
    fn from(window: TimeWindowDto) -> Self {
        Self {
            from: window.from,
            to: window.to,
        }
    }
}

impl From<ClaudeSettings> for ClaudeSettingsDto {
    fn from(claude: ClaudeSettings) -> Self {
        Self {
            command: claude.command,
            model: claude.model,
            effort: claude.effort,
            max_turns: claude.max_turns,
            timeout_minutes: claude.timeout_minutes,
            permission_mode: claude.permission_mode,
            allowed_tools: claude.allowed_tools,
            use_subscription: claude.use_subscription,
            extra_args: claude.extra_args,
            git_bash_path: claude.git_bash_path,
        }
    }
}

impl From<ClaudeSettingsDto> for ClaudeSettings {
    fn from(claude: ClaudeSettingsDto) -> Self {
        Self {
            command: claude.command,
            model: claude.model,
            effort: claude.effort,
            max_turns: claude.max_turns,
            timeout_minutes: claude.timeout_minutes,
            permission_mode: claude.permission_mode,
            allowed_tools: claude.allowed_tools,
            use_subscription: claude.use_subscription,
            extra_args: claude.extra_args,
            git_bash_path: claude.git_bash_path,
        }
    }
}

impl From<Profile> for ProfileDto {
    fn from(profile: Profile) -> Self {
        Self {
            id: profile.id,
            name: profile.name,
            model: profile.model,
            effort: profile.effort,
            max_turns: profile.max_turns,
            timeout_minutes: profile.timeout_minutes,
            accounts: profile.accounts,
        }
    }
}

impl From<ProfileDto> for Profile {
    fn from(profile: ProfileDto) -> Self {
        Self {
            id: profile.id,
            name: profile.name,
            model: profile.model,
            effort: profile.effort,
            max_turns: profile.max_turns,
            timeout_minutes: profile.timeout_minutes,
            accounts: profile.accounts,
        }
    }
}

impl From<Rule> for RuleDto {
    fn from(rule: Rule) -> Self {
        Self {
            id: rule.id,
            when: RuleConditionDto {
                labels: rule.when.labels,
                priorities: rule.when.priorities,
                files_min: rule.when.files_min,
                plan_min_chars: rule.when.plan_min_chars,
            },
            profile: rule.profile,
            accounts: rule.accounts,
        }
    }
}

impl From<RuleDto> for Rule {
    fn from(rule: RuleDto) -> Self {
        Self {
            id: rule.id,
            when: RuleCondition {
                labels: rule.when.labels,
                priorities: rule.when.priorities,
                files_min: rule.when.files_min,
                plan_min_chars: rule.when.plan_min_chars,
            },
            profile: rule.profile,
            accounts: rule.accounts,
        }
    }
}

impl From<DispatcherConfig> for DispatcherConfigDto {
    fn from(config: DispatcherConfig) -> Self {
        Self {
            enabled: config.enabled,
            wip: config.wip,
            window: config.window.map(Into::into),
            opt_in_label: config.opt_in_label,
            opt_out_label: config.opt_out_label,
            agent_prefix: config.agent_prefix,
            stale_minutes: config.stale_minutes,
            max_attempts: config.max_attempts,
            pause_minutes_on_limit: config.pause_minutes_on_limit,
            prompt_extra: config.prompt_extra,
            prompts: PromptTemplatesDto {
                work: config.prompts.work,
                plan: config.prompts.plan,
                research: config.prompts.research,
                merge: config.prompts.merge,
            },
            merge_strategy: config.merge_strategy.into(),
            review_before_merge: config.review_before_merge,
            join_worktrees: config.join_worktrees.into(),
            worktree_setup: config
                .worktree_setup
                .into_iter()
                .map(|step| WorktreeSetupStepDto {
                    cwd: step.cwd,
                    run: step.run,
                })
                .collect(),
            worktree_setup_timeout_minutes: config.worktree_setup_timeout_minutes,
            claude: config.claude.into(),
            profiles: config.profiles.into_iter().map(Into::into).collect(),
            default_profile: config.default_profile,
            rules: config.rules.into_iter().map(Into::into).collect(),
            escalate: config.escalate,
            ladder: config.ladder,
            allow_fable: config.allow_fable,
            research: ResearchSettingsDto {
                auto: config.research.auto,
                wip: config.research.wip,
                max_per_day: config.research.max_per_day,
                profile: config.research.profile,
            },
            planning: PlanningSettingsDto {
                auto: config.planning.auto,
                wip: config.planning.wip,
                acceptance: config.planning.acceptance,
                profile: config.planning.profile,
            },
            autopilot: AutopilotSettingsDto {
                enabled: config.autopilot.enabled,
                except_label: config.autopilot.except_label,
                max_merge_sessions: config.autopilot.max_merge_sessions,
            },
            account_strategy: config.account_strategy.into(),
            account_limit_percent: config.account_limit_percent,
            account_reserve_five_hour: config.account_reserve_five_hour,
            account_reserve_week: config.account_reserve_week,
            account_reset_grace_minutes: config.account_reset_grace_minutes,
        }
    }
}

impl From<DispatcherConfigDto> for DispatcherConfig {
    fn from(config: DispatcherConfigDto) -> Self {
        Self {
            enabled: config.enabled,
            wip: config.wip,
            window: config.window.map(Into::into),
            opt_in_label: config.opt_in_label,
            opt_out_label: config.opt_out_label,
            agent_prefix: config.agent_prefix,
            stale_minutes: config.stale_minutes,
            max_attempts: config.max_attempts,
            pause_minutes_on_limit: config.pause_minutes_on_limit,
            prompt_extra: config.prompt_extra,
            prompts: PromptTemplates {
                work: config.prompts.work,
                plan: config.prompts.plan,
                research: config.prompts.research,
                merge: config.prompts.merge,
            },
            merge_strategy: config.merge_strategy.into(),
            review_before_merge: config.review_before_merge,
            join_worktrees: config.join_worktrees.into(),
            worktree_setup: config
                .worktree_setup
                .into_iter()
                .map(|step| WorktreeSetupStep {
                    cwd: step.cwd,
                    run: step.run,
                })
                .collect(),
            worktree_setup_timeout_minutes: config.worktree_setup_timeout_minutes,
            claude: config.claude.into(),
            profiles: config.profiles.into_iter().map(Into::into).collect(),
            default_profile: config.default_profile,
            rules: config.rules.into_iter().map(Into::into).collect(),
            escalate: config.escalate,
            ladder: config.ladder,
            allow_fable: config.allow_fable,
            research: ResearchSettings {
                auto: config.research.auto,
                wip: config.research.wip,
                max_per_day: config.research.max_per_day,
                profile: config.research.profile,
            },
            planning: PlanningSettings {
                auto: config.planning.auto,
                wip: config.planning.wip,
                acceptance: config.planning.acceptance,
                profile: config.planning.profile,
            },
            autopilot: AutopilotSettings {
                enabled: config.autopilot.enabled,
                except_label: config.autopilot.except_label,
                max_merge_sessions: config.autopilot.max_merge_sessions,
            },
            account_strategy: config.account_strategy.into(),
            account_limit_percent: config.account_limit_percent,
            account_reserve_five_hour: config.account_reserve_five_hour,
            account_reserve_week: config.account_reserve_week,
            account_reset_grace_minutes: config.account_reset_grace_minutes,
        }
    }
}

impl From<ConfigView> for DispatcherConfigView {
    fn from(view: ConfigView) -> Self {
        Self {
            config: view.config.into(),
            hash: view.hash,
            updated_at: view.updated_at.as_millis(),
            version: view.version.value(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/dispatcher/config",
    tag = "dispatcher",
    responses((status = 200, body = DispatcherConfigView), (status = 401, body = Problem))
)]
pub(crate) async fn get_dispatcher_config(
    State(state): State<ApiState>,
) -> Result<Json<DispatcherConfigView>, ApiError> {
    Ok(Json(state.dispatcher_config.get().await?.into()))
}

#[utoipa::path(
    put,
    path = "/dispatcher/config",
    tag = "dispatcher",
    request_body = UpdateDispatcherConfig,
    responses(
        (status = 200, body = DispatcherConfigView),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "Invalid fields, listed in `errors`", body = Problem),
    )
)]
pub(crate) async fn update_dispatcher_config(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<UpdateDispatcherConfig>,
) -> Result<Json<DispatcherConfigView>, ApiError> {
    let expected = AppError::client_version(input.version)?;
    let view = state
        .dispatcher_config
        .update(input.config.into(), expected)
        .await?;
    Ok(Json(view.into()))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;

    use crate::tests::{TestApi, bearer, local, send, test_api};

    async fn call(
        api: &TestApi,
        request: axum::http::request::Builder,
        body: Option<serde_json::Value>,
    ) -> (u16, serde_json::Value) {
        let request = bearer(request, &api.token);
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    async fn get(api: &TestApi) -> serde_json::Value {
        let (status, view) = call(api, Request::get("/api/v1/dispatcher/config"), None).await;
        assert_eq!(status, 200);
        view
    }

    async fn put(
        api: &TestApi,
        version: i64,
        config: &serde_json::Value,
    ) -> (u16, serde_json::Value) {
        call(
            api,
            Request::put("/api/v1/dispatcher/config"),
            Some(json!({ "version": version, "config": config })),
        )
        .await
    }

    #[tokio::test]
    async fn shows_the_default_configuration_with_its_hash_and_version() {
        let api = test_api().await;
        let view = get(&api).await;
        let config = &view["config"];
        assert_eq!(view["version"], 1);
        assert_eq!(view["hash"].as_str().unwrap().len(), 16);
        assert_eq!(config["enabled"], false);
        assert_eq!(config["wip"], 1);
        assert_eq!(config["window"], serde_json::Value::Null);
        assert_eq!(config["optOutLabel"], "manual");
        assert_eq!(config["mergeStrategy"], "merge");
        assert_eq!(config["joinWorktrees"], "none");
        assert_eq!(config["accountStrategy"], "most-free");
        assert_eq!(config["staleMinutes"], 150);
        assert_eq!(config["defaultProfile"], "medium");
        assert_eq!(config["ladder"], json!(["small", "medium", "large"]));
        assert_eq!(config["profiles"][2]["maxTurns"], 150);
        assert_eq!(config["rules"][3]["when"]["filesMin"], 6);
        assert_eq!(
            config["claude"]["allowedTools"].as_array().unwrap().len(),
            16
        );
    }

    #[tokio::test]
    async fn saves_a_changed_configuration_and_answers_with_the_next_version() {
        let api = test_api().await;
        let mut config = get(&api).await["config"].clone();
        config["enabled"] = json!(true);
        config["wip"] = json!(2);
        config["window"] = json!({ "from": "08:00", "to": "18:00" });
        config["joinWorktrees"] = json!("same-spec");
        config["profiles"][0]["accounts"] = json!(["default"]);
        let (status, saved) = put(&api, 1, &config).await;
        assert_eq!(status, 200);
        assert_eq!(saved["version"], 2);
        assert_eq!(saved["config"], config);
        assert_eq!(get(&api).await["config"], config);

        let (status, same) = put(&api, 2, &config).await;
        assert_eq!(status, 200);
        assert_eq!(same["version"], 2);
    }

    #[tokio::test]
    async fn refuses_invalid_fields_with_a_list_of_them() {
        let api = test_api().await;
        let mut config = get(&api).await["config"].clone();
        config["wip"] = json!(-1);
        config["window"] = json!({ "from": "25:00", "to": "18:00" });
        config["profiles"][1]["maxTurns"] = json!(0);
        config["rules"][0]["profile"] = json!("giant");
        config["profiles"][0]["accounts"] = json!(["ghost"]);
        let (status, problem) = put(&api, 1, &config).await;
        assert_eq!(status, 422);
        assert_eq!(problem["code"], "dispatcher-config-invalid");
        let fields: Vec<&str> = problem["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|error| error["field"].as_str().unwrap())
            .collect();
        assert_eq!(
            fields,
            ["wip", "window", "profiles[1].maxTurns", "rules[0].profile"]
        );
        assert!(
            problem["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains("whole number")
        );

        config["wip"] = json!(1);
        config["window"] = serde_json::Value::Null;
        config["profiles"][1]["maxTurns"] = json!(80);
        config["rules"][0]["profile"] = json!("small");
        let (status, problem) = put(&api, 1, &config).await;
        assert_eq!(status, 422);
        assert_eq!(problem["errors"][0]["field"], "profiles[0].accounts[0]");
        assert_eq!(get(&api).await["version"], 1);
    }

    #[tokio::test]
    async fn refuses_a_stale_version_and_malformed_bodies() {
        let api = test_api().await;
        let mut config = get(&api).await["config"].clone();
        config["wip"] = json!(2);
        assert_eq!(put(&api, 1, &config).await.0, 200);
        let (status, problem) = put(&api, 1, &config).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (409, Some("version-conflict"))
        );
        let (status, problem) = put(&api, 0, &config).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("version-invalid"))
        );
        let (status, problem) = call(
            &api,
            Request::put("/api/v1/dispatcher/config"),
            Some(json!({ "version": 2, "config": { "enabled": true } })),
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("input-invalid"))
        );
        config["unknown"] = json!(1);
        assert_eq!(put(&api, 2, &config).await.0, 422);
    }

    #[tokio::test]
    async fn needs_a_login() {
        let api = test_api().await;
        let request = local(Request::get("/api/v1/dispatcher/config"))
            .body(Body::empty())
            .unwrap();
        let (status, _, _) = send(&api.router, request).await;
        assert_eq!(status, 401);
    }

    #[tokio::test]
    async fn announces_the_change_to_event_streams() {
        let api = test_api().await;
        let mut config = get(&api).await["config"].clone();
        config["wip"] = json!(3);
        put(&api, 1, &config).await;
        let changes = api.changes.changes_since(0, 10).await.unwrap();
        let ours = changes
            .iter()
            .filter(|change| change.entity == nullradix_app::Entity::DispatcherConfig)
            .count();
        assert_eq!(ours, 1);
    }
}
