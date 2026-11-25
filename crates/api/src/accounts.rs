use std::fmt;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use nullradix_app::AppError;
use nullradix_app::accounts::AccountPatch;
use nullradix_domain::{Account, Id};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::tickets::present;
use crate::{API_PREFIX, ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

/// A Claude account as the API shows it; the run token itself never leaves the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    pub id: Uuid,
    /// Short id used in labels such as `account:max` and on the command line.
    pub slug: String,
    pub name: String,
    /// Claude folder as typed; none for the default account, which uses the server login.
    pub config_dir: Option<String>,
    pub enabled: bool,
    /// Only used when no other allowed account is free.
    pub fallback: bool,
    /// Usage ceiling in percent for the five hour window; none uses the dispatcher's default.
    pub limit_five_hour: Option<i64>,
    /// Usage ceiling in percent for the week; none uses the dispatcher's default.
    pub limit_week: Option<i64>,
    /// Whether runs of this account start with a stored long-lived token.
    pub has_run_token: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: i64,
}

impl From<Account> for AccountDto {
    fn from(account: Account) -> Self {
        Self {
            id: account.id().as_uuid(),
            slug: account.slug().to_string(),
            name: account.name().as_str().to_owned(),
            config_dir: account.config_dir().map(|dir| dir.as_str().to_owned()),
            enabled: account.enabled(),
            fallback: account.fallback(),
            limit_five_hour: account
                .limit_five_hour()
                .map(|limit| i64::from(limit.percent())),
            limit_week: account.limit_week().map(|limit| i64::from(limit.percent())),
            has_run_token: account.token_ref().is_some(),
            created_at: account.created_at().as_millis(),
            updated_at: account.updated_at().as_millis(),
            version: account.version().value(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountList {
    pub items: Vec<AccountDto>,
    /// Folder where new accounts get their own subfolders.
    pub accounts_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateAccount {
    pub name: String,
    /// An existing Claude folder to take over, such as `~/.mcacc/work`; else a new one is made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
}

/// A change of fields; left out fields stay, `null` removes a limit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateAccount {
    /// Version the change is based on; a newer stored version fails with `version-conflict`.
    pub version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<bool>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<i64>, nullable, minimum = 1, maximum = 100)]
    pub limit_five_hour: Option<Option<i64>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<i64>, nullable, minimum = 1, maximum = 100)]
    pub limit_week: Option<Option<i64>>,
}

/// The long-lived token of `claude setup-token`; it is only ever sent, never returned.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetRunToken {
    #[schema(format = Password, example = "sk-ant-oat01-...")]
    pub token: String,
    /// Version the change is based on.
    pub version: i64,
}

impl fmt::Debug for SetRunToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetRunToken")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct VersionQuery {
    /// Version the change is based on.
    version: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub(crate) struct DeleteAccountQuery {
    /// Version the deletion is based on.
    version: i64,
    /// Also delete the account's own Claude folder, only allowed below the accounts folder.
    #[serde(default)]
    delete_dir: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Path, rename_all = "camelCase")]
pub(crate) struct AccountPath {
    /// Account id.
    pub(crate) account_id: Uuid,
}

#[utoipa::path(
    get,
    path = "/accounts",
    tag = "accounts",
    responses((status = 200, body = AccountList), (status = 401, body = Problem))
)]
pub(crate) async fn list_accounts(
    State(state): State<ApiState>,
) -> Result<Json<AccountList>, ApiError> {
    let items = state
        .accounts
        .list()
        .await?
        .into_iter()
        .map(AccountDto::from)
        .collect();
    Ok(Json(AccountList {
        items,
        accounts_dir: state.accounts.accounts_dir(),
    }))
}

#[utoipa::path(
    post,
    path = "/accounts",
    tag = "accounts",
    request_body = CreateAccount,
    responses(
        (status = 201, body = AccountDto,
            headers(("Location" = String, description = "URL of the new account"))),
        (status = 401, body = Problem),
        (status = 409, description = "The folder belongs to another account", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn create_account(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<CreateAccount>,
) -> Result<(StatusCode, HeaderMap, Json<AccountDto>), ApiError> {
    let account = state
        .accounts
        .create(&input.name, input.config_dir.as_deref())
        .await?;
    let account = AccountDto::from(account);
    let mut headers = HeaderMap::new();
    let location = format!("{API_PREFIX}/accounts/{}", account.id);
    if let Ok(value) = HeaderValue::from_str(&location) {
        headers.insert(header::LOCATION, value);
    }
    Ok((StatusCode::CREATED, headers, Json(account)))
}

#[utoipa::path(
    patch,
    path = "/accounts/{accountId}",
    tag = "accounts",
    params(AccountPath),
    request_body = UpdateAccount,
    responses(
        (status = 200, body = AccountDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn update_account(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<AccountPath>,
    ApiJson(input): ApiJson<UpdateAccount>,
) -> Result<Json<AccountDto>, ApiError> {
    let expected = AppError::client_version(input.version)?;
    let changes = AccountPatch {
        name: input.name,
        enabled: input.enabled,
        fallback: input.fallback,
        limit_five_hour: input.limit_five_hour,
        limit_week: input.limit_week,
    };
    let account = state
        .accounts
        .update(Id::from_uuid(path.account_id), changes, expected)
        .await?;
    Ok(Json(account.into()))
}

#[utoipa::path(
    delete,
    path = "/accounts/{accountId}",
    tag = "accounts",
    params(AccountPath, DeleteAccountQuery),
    responses(
        (status = 204, description = "The account is gone"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "The default account stays, or the folder is not the account's own",
            body = Problem),
    )
)]
pub(crate) async fn delete_account(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<AccountPath>,
    ApiQuery(query): ApiQuery<DeleteAccountQuery>,
) -> Result<StatusCode, ApiError> {
    let id = Id::from_uuid(path.account_id);
    let expected = AppError::client_version(query.version)?;
    // a running login holds files of the folder, so it ends first, if the removal can go on
    if state.accounts.get(id).await?.version() == expected {
        state.logins.cancel(id, false);
    }
    state
        .accounts
        .remove(id, expected, query.delete_dir)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/accounts/{accountId}/run-token",
    tag = "accounts",
    params(AccountPath),
    request_body = SetRunToken,
    responses(
        (status = 200, body = AccountDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "The token does not look like one of `claude setup-token`",
            body = Problem),
        (status = 503, description = "The secret store did not answer", body = Problem),
    )
)]
pub(crate) async fn set_run_token(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<AccountPath>,
    ApiJson(input): ApiJson<SetRunToken>,
) -> Result<Json<AccountDto>, ApiError> {
    let account = state
        .accounts
        .set_token(
            Id::from_uuid(path.account_id),
            &input.token,
            AppError::client_version(input.version)?,
        )
        .await?;
    Ok(Json(account.into()))
}

#[utoipa::path(
    delete,
    path = "/accounts/{accountId}/run-token",
    tag = "accounts",
    params(AccountPath, VersionQuery),
    responses(
        (status = 200, body = AccountDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn clear_run_token(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<AccountPath>,
    ApiQuery(query): ApiQuery<VersionQuery>,
) -> Result<Json<AccountDto>, ApiError> {
    let account = state
        .accounts
        .clear_token(
            Id::from_uuid(path.account_id),
            AppError::client_version(query.version)?,
        )
        .await?;
    Ok(Json(account.into()))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;

    use crate::tests::{TestApi, bearer, send, test_api};

    const TOKEN: &str = "sk-ant-oat01-fake-token";

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

    fn problem(status: axum::http::StatusCode, json: &serde_json::Value) -> (u16, &str) {
        (status.as_u16(), json["code"].as_str().unwrap_or_default())
    }

    #[tokio::test]
    async fn lists_the_default_account_and_the_accounts_folder() {
        let api = test_api().await;
        let (status, list) = call(&api, Request::get("/api/v1/accounts"), None).await;
        assert_eq!(status, 200);
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
        let default = &list["items"][0];
        assert_eq!(default["slug"], "default");
        assert_eq!(default["configDir"], serde_json::Value::Null);
        assert_eq!(default["hasRunToken"], false);
        assert_eq!(default["enabled"], true);
        assert!(list["accountsDir"].as_str().is_some());
    }

    #[tokio::test]
    async fn creates_updates_and_deletes_accounts() {
        let api = test_api().await;
        let (status, created) = call(
            &api,
            Request::post("/api/v1/accounts"),
            Some(json!({ "name": "Max" })),
        )
        .await;
        assert_eq!(status, 201);
        assert_eq!(created["slug"], "max");
        assert_eq!(created["version"], 1);
        let id = created["id"].as_str().unwrap().to_owned();
        let dir = created["configDir"].as_str().unwrap().to_owned();
        assert!(std::path::Path::new(&dir).is_dir());

        let (status, updated) = call(
            &api,
            Request::patch(format!("/api/v1/accounts/{id}")),
            Some(json!({ "version": 1, "enabled": false, "limitWeek": 80, "limitFiveHour": 50 })),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(updated["enabled"], false);
        assert_eq!(updated["limitWeek"], 80);
        assert_eq!(updated["version"], 2);

        let (_, cleared) = call(
            &api,
            Request::patch(format!("/api/v1/accounts/{id}")),
            Some(json!({ "version": 2, "limitWeek": null })),
        )
        .await;
        assert_eq!(cleared["limitWeek"], serde_json::Value::Null);
        assert_eq!(cleared["limitFiveHour"], 50);

        let (status, stale) = call(
            &api,
            Request::delete(format!("/api/v1/accounts/{id}?version=1")),
            None,
        )
        .await;
        assert_eq!(problem(status, &stale), (409, "version-conflict"));
        let (status, _) = call(
            &api,
            Request::delete(format!("/api/v1/accounts/{id}?version=3&deleteDir=true")),
            None,
        )
        .await;
        assert_eq!(status, 204);
        assert!(!std::path::Path::new(&dir).exists());
        let (_, list) = call(&api, Request::get("/api/v1/accounts"), None).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn removing_an_account_ends_its_running_login_first() {
        let api = test_api().await;
        let (_, created) = call(
            &api,
            Request::post("/api/v1/accounts"),
            Some(json!({ "name": "Max" })),
        )
        .await;
        let id = created["id"].as_str().unwrap().to_owned();
        let uuid: uuid::Uuid = id.parse().unwrap();
        let account = nullradix_domain::Id::from_uuid(uuid);
        let (status, _) = call(
            &api,
            Request::post(format!("/api/v1/accounts/{id}/login")),
            Some(json!({})),
        )
        .await;
        assert_eq!(status, 202);
        assert!(api.logins.status(account).is_some());

        let (status, _) = call(
            &api,
            Request::delete(format!("/api/v1/accounts/{id}?version=7")),
            None,
        )
        .await;
        assert_eq!(status, 409);
        assert_eq!(
            api.logins.status(account).map(|login| login.state),
            Some(nullradix_app::login::LoginState::Running)
        );

        let (status, _) = call(
            &api,
            Request::delete(format!("/api/v1/accounts/{id}?version=1&deleteDir=true")),
            None,
        )
        .await;
        assert_eq!(status, 204);
        assert!(api.logins.status(account).is_none());
    }
    #[tokio::test]
    async fn validates_input_with_problem_details() {
        let api = test_api().await;
        let (status, json) = call(
            &api,
            Request::post("/api/v1/accounts"),
            Some(json!({ "name": " " })),
        )
        .await;
        assert_eq!(problem(status, &json), (422, "account-invalid"));
        let (status, json) = call(
            &api,
            Request::post("/api/v1/accounts"),
            Some(json!({ "name": "Max", "extra": 1 })),
        )
        .await;
        assert_eq!(problem(status, &json), (422, "input-invalid"));
        let (status, json) = call(
            &api,
            Request::post("/api/v1/accounts"),
            Some(json!({ "name": "Max", "configDir": "relative/dir" })),
        )
        .await;
        assert_eq!(problem(status, &json), (422, "account-invalid"));
        let (_, list) = call(&api, Request::get("/api/v1/accounts"), None).await;
        let default = list["items"][0]["id"].as_str().unwrap().to_owned();
        let (status, json) = call(
            &api,
            Request::patch(format!("/api/v1/accounts/{default}")),
            Some(json!({ "version": 1, "limitWeek": 101 })),
        )
        .await;
        assert_eq!(problem(status, &json), (422, "account-invalid"));
        let (status, json) = call(
            &api,
            Request::patch(format!("/api/v1/accounts/{default}")),
            Some(json!({ "version": 0 })),
        )
        .await;
        assert_eq!(problem(status, &json), (422, "version-invalid"));
        let (status, json) = call(
            &api,
            Request::delete(format!("/api/v1/accounts/{default}?version=1")),
            None,
        )
        .await;
        assert_eq!(problem(status, &json), (422, "account-invalid"));
        let (status, json) = call(
            &api,
            Request::patch("/api/v1/accounts/00000000-0000-0000-0000-000000000009"),
            Some(json!({ "version": 1 })),
        )
        .await;
        assert_eq!(problem(status, &json), (404, "account-not-found"));
        let (status, json) = call(
            &api,
            Request::patch("/api/v1/accounts/nope"),
            Some(json!({})),
        )
        .await;
        assert_eq!(problem(status, &json), (422, "path-invalid"));
    }

    #[tokio::test]
    async fn run_tokens_are_stored_but_never_returned() {
        let api = test_api().await;
        let (_, created) = call(
            &api,
            Request::post("/api/v1/accounts"),
            Some(json!({ "name": "Max" })),
        )
        .await;
        let id = created["id"].as_str().unwrap().to_owned();
        let (status, set) = call(
            &api,
            Request::put(format!("/api/v1/accounts/{id}/run-token")),
            Some(json!({ "token": TOKEN, "version": 1 })),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(set["hasRunToken"], true);
        assert!(!set.to_string().contains(TOKEN));
        assert!(!set.to_string().contains("secret"));
        let (_, list) = call(&api, Request::get("/api/v1/accounts"), None).await;
        assert!(!list.to_string().contains(TOKEN));
        assert!(!list.to_string().contains("account-token"));

        let (status, bad) = call(
            &api,
            Request::put(format!("/api/v1/accounts/{id}/run-token")),
            Some(json!({ "token": "sk-ant-api03-not-a-run-token", "version": 2 })),
        )
        .await;
        assert_eq!(problem(status, &bad), (422, "run-token-invalid"));
        assert!(!bad.to_string().contains("sk-ant-api03"));

        let (status, cleared) = call(
            &api,
            Request::delete(format!("/api/v1/accounts/{id}/run-token?version=2")),
            None,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(cleared["hasRunToken"], false);
        assert_eq!(cleared["version"], 3);
    }

    #[test]
    fn the_token_body_never_shows_in_debug_output() {
        let body = super::SetRunToken {
            token: String::from(TOKEN),
            version: 1,
        };
        assert!(!format!("{body:?}").contains(TOKEN));
    }
}
