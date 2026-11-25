use axum::Json;
use axum::extract::State;
use nullradix_app::login::LoginStatus;
use nullradix_app::usage::{AccountStatus, AccountUsage, UsageReason, UsageWindow};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::login::LoginDto;
use crate::{ApiError, ApiQuery, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum UsageReasonDto {
    Disabled,
    NoLogin,
    TokenExpired,
    RequestFailed,
    RateLimited,
}

impl From<UsageReason> for UsageReasonDto {
    fn from(reason: UsageReason) -> Self {
        match reason {
            UsageReason::Disabled => Self::Disabled,
            UsageReason::NoLogin => Self::NoLogin,
            UsageReason::TokenExpired => Self::TokenExpired,
            UsageReason::RequestFailed => Self::RequestFailed,
            UsageReason::RateLimited => Self::RateLimited,
        }
    }
}

/// How much of a limit window is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindowDto {
    pub percent: i64,
    /// When the window starts over, in Unix milliseconds.
    pub resets_at: Option<i64>,
}

impl From<UsageWindow> for UsageWindowDto {
    fn from(window: UsageWindow) -> Self {
        Self {
            percent: i64::from(window.percent),
            resets_at: window.resets_at.map(nullradix_domain::Timestamp::as_millis),
        }
    }
}

/// The limits of the subscription behind an account; without numbers `reason` says why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsageDto {
    pub available: bool,
    pub reason: Option<UsageReasonDto>,
    /// Subscription plan such as `max`.
    pub plan: Option<String>,
    pub five_hour: Option<UsageWindowDto>,
    pub seven_day: Option<UsageWindowDto>,
    /// When the numbers were read, in Unix milliseconds.
    pub fetched_at: Option<i64>,
}

impl From<AccountUsage> for UsageDto {
    fn from(usage: AccountUsage) -> Self {
        Self {
            available: usage.available(),
            reason: usage.reason.map(Into::into),
            plan: usage.plan,
            five_hour: usage.five_hour.map(Into::into),
            seven_day: usage.seven_day.map(Into::into),
            fetched_at: usage.fetched_at.map(nullradix_domain::Timestamp::as_millis),
        }
    }
}

/// What is known about an account's Claude login and limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatusDto {
    pub account_id: Uuid,
    pub slug: String,
    pub logged_in: bool,
    pub email: Option<String>,
    /// The folder Claude Code uses for the account.
    pub folder: String,
    pub usage: UsageDto,
    /// The latest `claude auth login` of the account, while the daemon remembers it.
    pub login: Option<LoginDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AccountStatusList {
    pub items: Vec<AccountStatusDto>,
}

impl AccountStatusDto {
    fn new(status: AccountStatus, slug: String, login: Option<LoginStatus>) -> Self {
        Self {
            account_id: status.account.as_uuid(),
            slug,
            logged_in: status.logged_in,
            email: status.email,
            folder: status.folder,
            usage: status.usage.into(),
            login: login.map(Into::into),
        }
    }
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct UsageQuery {
    /// Asks the endpoint again instead of using a reading of the last minute.
    #[serde(default)]
    fresh: bool,
}

#[utoipa::path(
    get,
    path = "/accounts/usage",
    tag = "accounts",
    params(UsageQuery),
    responses(
        (status = 200, body = AccountStatusList),
        (status = 401, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn account_usage(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<UsageQuery>,
) -> Result<Json<AccountStatusList>, ApiError> {
    let accounts = state.accounts.list().await?;
    let statuses = state.usage.status(&accounts, query.fresh).await;
    let items = statuses
        .into_iter()
        .zip(&accounts)
        .map(|(status, account)| {
            let login = state.logins.status(account.id());
            AccountStatusDto::new(status, account.slug().to_string(), login)
        })
        .collect();
    Ok(Json(AccountStatusList { items }))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;

    use crate::tests::{TestApi, bearer, send, test_api};

    async fn get(api: &TestApi, path: &str) -> (axum::http::StatusCode, serde_json::Value) {
        let request = bearer(Request::get(path), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status, json)
    }

    #[tokio::test]
    async fn shows_the_login_and_the_limits_of_every_account() {
        let api = test_api().await;
        let (status, json) = get(&api, "/api/v1/accounts/usage").await;
        assert_eq!(status, 200);
        let items = json["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        let first = &items[0];
        assert_eq!(first["slug"], "default");
        assert_eq!(first["loggedIn"], true);
        assert_eq!(first["email"], "me@example.com");
        assert_eq!(first["usage"]["available"], true);
        assert_eq!(first["usage"]["plan"], "max");
        assert_eq!(first["usage"]["fiveHour"]["percent"], 42);
        assert_eq!(
            first["usage"]["fiveHour"]["resetsAt"],
            1_900_000_000_000_i64
        );
        assert_eq!(
            first["usage"]["sevenDay"]["resetsAt"],
            serde_json::Value::Null
        );
    }

    #[tokio::test]
    async fn disabled_accounts_show_why_there_are_no_numbers() {
        let api = test_api().await;
        let default = api.accounts.find("default").await.unwrap();
        api.accounts
            .update(
                default.id(),
                nullradix_app::accounts::AccountPatch {
                    enabled: Some(false),
                    ..Default::default()
                },
                default.version(),
            )
            .await
            .unwrap();
        let (_, json) = get(&api, "/api/v1/accounts/usage?fresh=true").await;
        assert_eq!(json["items"][0]["usage"]["available"], false);
        assert_eq!(json["items"][0]["usage"]["reason"], "disabled");
    }

    #[tokio::test]
    async fn a_running_login_shows_next_to_the_limits() {
        let api = test_api().await;
        let default = api.accounts.find("default").await.unwrap();
        let (_, before) = get(&api, "/api/v1/accounts/usage").await;
        assert_eq!(before["items"][0]["login"], serde_json::Value::Null);
        let request = bearer(
            Request::post(format!("/api/v1/accounts/{}/login", default.id())),
            &api.token,
        )
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
        let (status, _, _) = send(&api.router, request).await;
        assert_eq!(status, 202);
        let (_, after) = get(&api, "/api/v1/accounts/usage").await;
        assert_eq!(after["items"][0]["login"]["state"], "running");
    }
    #[tokio::test]
    async fn refuses_a_bad_flag() {
        let api = test_api().await;
        let (status, json) = get(&api, "/api/v1/accounts/usage?fresh=maybe").await;
        assert_eq!(
            (status.as_u16(), json["code"].as_str()),
            (422, Some("query-invalid"))
        );
    }
}
