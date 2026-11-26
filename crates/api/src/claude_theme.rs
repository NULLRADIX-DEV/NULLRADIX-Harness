//! The switch that puts the Claude part of the chosen theme into the Claude config folders.

use axum::Json;
use axum::extract::State;
use nullradix_app::claude_theme::{ClaudeThemeStatus, TargetState, TargetStatus};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, ApiJson, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ClaudeThemeStateDto {
    /// A folder holds the Claude part of the chosen theme.
    Active,
    /// Nothing is wanted: the switch is off or the chosen theme has no Claude part.
    None,
    /// A folder is not owned by NULLRADIX and stays as it is.
    Foreign,
    /// A folder could not be written.
    Error,
}

impl From<TargetState> for ClaudeThemeStateDto {
    fn from(state: TargetState) -> Self {
        match state {
            TargetState::Active => Self::Active,
            TargetState::None => Self::None,
            TargetState::Foreign => Self::Foreign,
            TargetState::Error => Self::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ClaudeThemeTargetDto {
    /// Claude config folder: the default one or the folder of an account.
    pub path: String,
    pub state: ClaudeThemeStateDto,
    /// Why the folder is `foreign` or in `error`.
    pub message: Option<String>,
}

impl From<TargetStatus> for ClaudeThemeTargetDto {
    fn from(target: TargetStatus) -> Self {
        Self {
            path: target.path,
            state: target.state.into(),
            message: target.message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeThemeDto {
    /// Whether the person turned the switch on; it is off until then.
    pub enabled: bool,
    /// The worst state of the folders.
    pub state: ClaudeThemeStateDto,
    /// The chosen theme, when it is installed.
    pub theme_id: Option<String>,
    pub theme_name: Option<String>,
    pub targets: Vec<ClaudeThemeTargetDto>,
}

impl From<ClaudeThemeStatus> for ClaudeThemeDto {
    fn from(status: ClaudeThemeStatus) -> Self {
        Self {
            enabled: status.enabled,
            state: status.state.into(),
            theme_id: status.theme_id,
            theme_name: status.theme_name,
            targets: status.targets.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetClaudeTheme {
    /// Put the Claude part of the chosen theme into the Claude config folders, or take it out.
    pub enabled: bool,
}

/// Brings the folders in line after a change that may affect them; a failure only goes to the log.
pub(crate) async fn resync(state: &ApiState) {
    if let Err(error) = state.claude_theme.reconcile().await {
        tracing::warn!(%error, "cannot sync the Claude theme");
    }
}

#[utoipa::path(
    get,
    path = "/settings/claude-theme",
    tag = "themes",
    responses((status = 200, body = ClaudeThemeDto), (status = 401, body = Problem))
)]
pub(crate) async fn claude_theme(
    State(state): State<ApiState>,
) -> Result<Json<ClaudeThemeDto>, ApiError> {
    Ok(Json(state.claude_theme.status().await?.into()))
}

#[utoipa::path(
    put,
    path = "/settings/claude-theme",
    tag = "themes",
    request_body = SetClaudeTheme,
    responses(
        (status = 200, description = "The switch is set and the folders are in line; a folder that failed shows in its state", body = ClaudeThemeDto),
        (status = 401, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn set_claude_theme(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<SetClaudeTheme>,
) -> Result<Json<ClaudeThemeDto>, ApiError> {
    Ok(Json(
        state.claude_theme.set_enabled(input.enabled).await?.into(),
    ))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use serde_json::{Value, json};

    use crate::tests::{TestApi, bearer, local, send, test_api};

    async fn call(api: &TestApi, method: &str, body: Option<Value>) -> (u16, Value) {
        let request = bearer(
            Request::builder()
                .method(method)
                .uri("/api/v1/settings/claude-theme"),
            &api.token,
        )
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    #[tokio::test]
    async fn the_switch_is_off_until_it_is_turned_on_and_stays() {
        let api = test_api().await;
        let (status, off) = call(&api, "GET", None).await;
        assert_eq!(status, 200);
        assert_eq!(
            off,
            json!({
                "enabled": false, "state": "none", "themeId": null,
                "themeName": null, "targets": []
            })
        );
        let (status, on) = call(&api, "PUT", Some(json!({ "enabled": true }))).await;
        assert_eq!((status, &on["enabled"]), (200, &json!(true)));
        let (_, again) = call(&api, "GET", None).await;
        assert_eq!(again["enabled"], true);
        let (_, back) = call(&api, "PUT", Some(json!({ "enabled": false }))).await;
        assert_eq!(back["enabled"], false);
    }

    #[tokio::test]
    async fn a_body_that_is_not_the_switch_is_refused() {
        let api = test_api().await;
        for body in [
            json!({}),
            json!({ "enabled": "yes" }),
            json!({ "enabled": true, "x": 1 }),
        ] {
            let (status, problem) = call(&api, "PUT", Some(body)).await;
            assert_eq!(status, 422, "{problem}");
        }
    }

    #[tokio::test]
    async fn the_switch_needs_a_login() {
        let api = test_api().await;
        for request in [
            local(Request::get("/api/v1/settings/claude-theme")),
            local(Request::put("/api/v1/settings/claude-theme")),
        ] {
            let (status, _, _) = send(&api.router, request.body(Body::empty()).unwrap()).await;
            assert_eq!(status, 401);
        }
    }
}
