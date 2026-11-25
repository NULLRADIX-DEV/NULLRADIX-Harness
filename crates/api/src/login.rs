use std::fmt;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use nullradix_app::login::{LoginState, LoginStatus, login_not_found};
use nullradix_domain::Id;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::accounts::{AccountDto, AccountPath};
use crate::{ApiError, ApiJson, ApiPath, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LoginStateDto {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl From<LoginState> for LoginStateDto {
    fn from(state: LoginState) -> Self {
        match state {
            LoginState::Running => Self::Running,
            LoginState::Done => Self::Done,
            LoginState::Failed => Self::Failed,
            LoginState::Cancelled => Self::Cancelled,
        }
    }
}

/// How a `claude auth login` of an account stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginDto {
    pub state: LoginStateDto,
    pub started_at: i64,
    /// The login page, for when no browser opened.
    pub url: Option<String>,
    /// The end of what Claude Code printed; the reason when the login failed.
    pub output: String,
}

impl From<LoginStatus> for LoginDto {
    fn from(status: LoginStatus) -> Self {
        Self {
            state: status.state.into(),
            started_at: status.started_at.as_millis(),
            url: status.url,
            output: status.output,
        }
    }
}

/// A new account that is logged in right away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountLoginDto {
    pub account: AccountDto,
    pub login: LoginDto,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartLogin {
    /// Filled into the login page of the browser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateAccountLogin {
    /// Without a name the account is named after its email once it is logged in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// The code the browser shows after logging in.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoginCode {
    #[schema(format = Password)]
    pub code: String,
}

impl fmt::Debug for LoginCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginCode").finish_non_exhaustive()
    }
}

#[utoipa::path(
    post,
    path = "/accounts/login",
    tag = "accounts",
    request_body = CreateAccountLogin,
    responses(
        (status = 202, description = "The account is added and its login runs", body = AccountLoginDto),
        (status = 401, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn create_account_login(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<CreateAccountLogin>,
) -> Result<(StatusCode, Json<AccountLoginDto>), ApiError> {
    let (account, login) = state
        .logins
        .create_and_login(
            input.name.as_deref().unwrap_or_default(),
            input.email.as_deref(),
        )
        .await?;
    let body = AccountLoginDto {
        account: account.into(),
        login: login.into(),
    };
    Ok((StatusCode::ACCEPTED, Json(body)))
}

#[utoipa::path(
    post,
    path = "/accounts/{accountId}/login",
    tag = "accounts",
    params(AccountPath),
    request_body = StartLogin,
    responses(
        (status = 202, description = "The login runs; a running one is ended first", body = LoginDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn start_login(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<AccountPath>,
    ApiJson(input): ApiJson<StartLogin>,
) -> Result<(StatusCode, Json<LoginDto>), ApiError> {
    let account = state.accounts.get(Id::from_uuid(path.account_id)).await?;
    let login = state.logins.login(&account, input.email.as_deref()).await?;
    Ok((StatusCode::ACCEPTED, Json(login.into())))
}

#[utoipa::path(
    get,
    path = "/accounts/{accountId}/login",
    tag = "accounts",
    params(AccountPath),
    responses(
        (status = 200, body = LoginDto),
        (status = 401, body = Problem),
        (status = 404, description = "No such account, or it had no login yet", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn get_login(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<AccountPath>,
) -> Result<Json<LoginDto>, ApiError> {
    let account = state.accounts.get(Id::from_uuid(path.account_id)).await?;
    state
        .logins
        .status(account.id())
        .map(|status| Json(status.into()))
        .ok_or_else(|| login_not_found(account.slug()).into())
}

#[utoipa::path(
    delete,
    path = "/accounts/{accountId}/login",
    tag = "accounts",
    params(AccountPath),
    responses(
        (status = 204, description = "The login is ended, or there was none"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn cancel_login(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<AccountPath>,
) -> Result<StatusCode, ApiError> {
    let account = state.accounts.get(Id::from_uuid(path.account_id)).await?;
    state.logins.cancel(account.id(), true);
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/accounts/{accountId}/login/code",
    tag = "accounts",
    params(AccountPath),
    request_body = LoginCode,
    responses(
        (status = 204, description = "The code went to the running login"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "No login runs for the account", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn send_login_code(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<AccountPath>,
    ApiJson(input): ApiJson<LoginCode>,
) -> Result<StatusCode, ApiError> {
    let account = state.accounts.get(Id::from_uuid(path.account_id)).await?;
    state.logins.code(&account, &input.code)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;

    use crate::tests::{TestApi, bearer, send, test_api};

    async fn call(
        api: &TestApi,
        request: axum::http::request::Builder,
        body: Option<serde_json::Value>,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        let request = bearer(request, &api.token);
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status, json)
    }

    #[tokio::test]
    async fn a_login_is_started_followed_fed_a_code_and_ended() {
        let api = test_api().await;
        let (_, created) = call(
            &api,
            Request::post("/api/v1/accounts"),
            Some(json!({ "name": "Max" })),
        )
        .await;
        let id = created["id"].as_str().unwrap().to_owned();
        let login = format!("/api/v1/accounts/{id}/login");

        let (status, missing) = call(&api, Request::get(&login), None).await;
        assert_eq!(
            (status.as_u16(), missing["code"].as_str()),
            (404, Some("login-not-found"))
        );
        let (status, idle) = call(
            &api,
            Request::post(format!("{login}/code")),
            Some(json!({ "code": "abc" })),
        )
        .await;
        assert_eq!(
            (status.as_u16(), idle["code"].as_str()),
            (409, Some("account-login-not-running"))
        );

        let (status, started) = call(&api, Request::post(&login), Some(json!({}))).await;
        assert_eq!(status, 202);
        assert_eq!(started["state"], "running");
        let (status, shown) = call(&api, Request::get(&login), None).await;
        assert_eq!(status, 200);
        assert!(shown["startedAt"].as_i64().unwrap() > 0);

        let (status, _) = call(
            &api,
            Request::post(format!("{login}/code")),
            Some(json!({ "code": " the-code " })),
        )
        .await;
        assert_eq!(status, 204);
        let (status, blank) = call(
            &api,
            Request::post(format!("{login}/code")),
            Some(json!({ "code": " " })),
        )
        .await;
        assert_eq!(
            (status.as_u16(), blank["code"].as_str()),
            (422, Some("input-invalid"))
        );

        let (status, _) = call(&api, Request::delete(&login), None).await;
        assert_eq!(status, 204);
        let (_, ended) = call(&api, Request::get(&login), None).await;
        assert_eq!(ended["state"], "cancelled");
    }

    #[tokio::test]
    async fn an_account_can_be_added_and_logged_in_in_one_step() {
        let api = test_api().await;
        let (status, body) = call(
            &api,
            Request::post("/api/v1/accounts/login"),
            Some(json!({ "email": "me@example.com" })),
        )
        .await;
        assert_eq!(status, 202);
        assert_eq!(body["account"]["name"], "me@example.com");
        assert_eq!(body["login"]["state"], "running");

        let (status, bad) = call(
            &api,
            Request::post("/api/v1/accounts/login"),
            Some(json!({ "email": "me@example.com & calc" })),
        )
        .await;
        assert_eq!(
            (status.as_u16(), bad["code"].as_str()),
            (422, Some("account-invalid"))
        );
        let (_, list) = call(&api, Request::get("/api/v1/accounts"), None).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn unknown_accounts_have_no_login_routes() {
        let api = test_api().await;
        let ghost = "/api/v1/accounts/00000000-0000-0000-0000-000000000009/login";
        for request in [Request::get(ghost), Request::delete(ghost)] {
            let (status, body) = call(&api, request, None).await;
            assert_eq!(
                (status.as_u16(), body["code"].as_str()),
                (404, Some("account-not-found"))
            );
        }
        let (status, body) = call(&api, Request::post(ghost), Some(json!({}))).await;
        assert_eq!(
            (status.as_u16(), body["code"].as_str()),
            (404, Some("account-not-found"))
        );
    }

    #[test]
    fn the_code_never_shows_in_debug_output() {
        let code = super::LoginCode {
            code: String::from("one-time-code-123"),
        };
        assert!(!format!("{code:?}").contains("one-time-code"));
    }
}
