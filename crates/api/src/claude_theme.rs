//! The switch that puts the Claude part of the chosen theme into the Claude config folders.

use axum::extract::State;
use axum::{Extension, Json};
use nullradix_app::auth::Principal;
use nullradix_app::claude_theme::{ClaudeThemeStatus, TargetState, TargetStatus, needs_person};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{AgentContext, require_person};
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
        (status = 403, description = "`claude-theme-needs-person`: a token or an agent tried to turn the switch on; only the person in the app may, a token may turn it off", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn set_claude_theme(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Extension(context): Extension<AgentContext>,
    ApiJson(input): ApiJson<SetClaudeTheme>,
) -> Result<Json<ClaudeThemeDto>, ApiError> {
    if input.enabled {
        require_person(&principal, context, needs_person)?;
    }
    Ok(Json(
        state.claude_theme.set_enabled(input.enabled).await?.into(),
    ))
}

#[cfg(test)]
pub(crate) mod fake {
    use async_trait::async_trait;
    use nullradix_app::claude_theme::{
        ClaudeSlots, SlotApplied, SlotFailure, SlotState, SlotTheme,
    };
    use parking_lot::Mutex;

    /// Records every apply and clear; the default folder is always a target.
    #[derive(Default)]
    pub(crate) struct Slots(Mutex<Vec<String>>);

    impl Slots {
        pub(crate) fn calls(&self) -> usize {
            self.0.lock().len()
        }

        pub(crate) fn recorded(&self) -> Vec<String> {
            self.0.lock().clone()
        }
    }

    #[async_trait]
    impl ClaudeSlots for Slots {
        fn default_dir(&self) -> Option<String> {
            Some(String::from("/default"))
        }

        fn key(&self, dir: &str) -> String {
            dir.to_owned()
        }

        fn settings_hash(&self, _theme: &SlotTheme) -> String {
            String::new()
        }

        async fn inspect(&self, _dir: &str) -> Result<SlotState, SlotFailure> {
            Ok(SlotState::Empty)
        }

        async fn apply(&self, dir: &str, theme: &SlotTheme) -> Result<SlotApplied, SlotFailure> {
            self.0.lock().push(format!("apply {dir} {}", theme.id));
            Ok(SlotApplied::default())
        }

        async fn clear(&self, dir: &str) -> Result<bool, SlotFailure> {
            self.0.lock().push(format!("clear {dir}"));
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};

    use crate::tests::{
        TestApi, as_person, bearer, in_run, in_session, local, open_session, send, test_api,
    };

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

    /// The person in the browser; only they turn the switch on.
    async fn person_put(api: &TestApi, body: Value) -> (u16, Value) {
        let cookie = open_session(api).await;
        let request = as_person(Request::put("/api/v1/settings/claude-theme"), &cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
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
                "themeName": null,
                "targets": [{ "path": "/default", "state": "none", "message": null }]
            })
        );
        let (status, on) = person_put(&api, json!({ "enabled": true })).await;
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
            let (status, problem) = person_put(&api, body).await;
            assert_eq!(status, 422, "{problem}");
        }
    }

    #[tokio::test]
    async fn only_the_person_turns_the_switch_on_and_a_token_only_off() {
        let api = test_api().await;
        let on = json!({ "enabled": true });
        // a token is refused, and so is a session that names a run or a session of an agent
        let (status, problem) = call(&api, "PUT", Some(on.clone())).await;
        assert_eq!(
            (status, &problem["code"]),
            (403, &json!("claude-theme-needs-person"))
        );
        let cookie = open_session(&api).await;
        for mark in [in_run, in_session] {
            let request = mark(as_person(
                Request::put("/api/v1/settings/claude-theme"),
                &cookie,
            ))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(on.to_string()))
            .unwrap();
            let (status, _, problem) = send(&api.router, request).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(problem["code"], "claude-theme-needs-person");
        }
        assert_eq!(call(&api, "GET", None).await.1["enabled"], false);
        assert_eq!(api.slots.calls(), 0);

        let (status, set) = person_put(&api, on).await;
        assert_eq!((status, &set["enabled"]), (200, &json!(true)));
        // a token may switch it off, and an off that changes nothing is fine as well
        let (status, set) = call(&api, "PUT", Some(json!({ "enabled": false }))).await;
        assert_eq!((status, &set["enabled"]), (200, &json!(false)));
        let (status, _) = call(&api, "PUT", Some(json!({ "enabled": false }))).await;
        assert_eq!(status, 200);
        // a refused body is still told apart from a refused person
        let (status, _) = call(&api, "PUT", Some(json!({ "enabled": "yes" }))).await;
        assert_eq!(status, 422);
    }

    #[tokio::test]
    async fn a_token_choosing_a_theme_with_a_claude_part_turns_the_switch_off() {
        let api = test_api().await;
        upload(&api).await;
        person_put(&api, json!({ "enabled": true })).await;
        let choose = |theme: &'static str| {
            call_at(
                &api,
                "PUT",
                "/api/v1/settings/theme",
                Some(json!({ "theme": theme })),
            )
        };
        // the person's own choice keeps the switch
        let cookie = open_session(&api).await;
        let request = as_person(Request::put("/api/v1/settings/theme"), &cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "theme": "night-blue" }).to_string()))
            .unwrap();
        assert_eq!(send(&api.router, request).await.0, StatusCode::OK);
        assert_eq!(call(&api, "GET", None).await.1["enabled"], true);
        // a token's choice of a theme without one does too
        assert_eq!(choose("light").await.0, 200);
        assert_eq!(call(&api, "GET", None).await.1["enabled"], true);
        // its choice of the theme with a Claude part does not
        assert_eq!(choose("night-blue").await.0, 200);
        let (_, status) = call(&api, "GET", None).await;
        assert_eq!(
            (&status["enabled"], &status["state"]),
            (&json!(false), &json!("none"))
        );
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

    const THEME: &str = r##"{
        "id": "night-blue", "name": "Night Blue", "version": "1.2.0", "author": "Ada",
        "sdkVersion": "^1.3", "colorScheme": "dark", "startColor": "#101018",
        "slots": [], "claudeTheme": true, "permissions": { "network": [] },
        "settingsSchema": { "dim": { "type": "boolean", "label": "Dim", "default": true } }
    }"##;

    async fn upload(api: &TestApi) {
        let bytes = format!(
            "manifest.json={THEME}\n--\ntheme.css=a {{}}\n--\nclaude/hooks/hooks.json={{}}"
        )
        .into_bytes();
        let cookie = open_session(api).await;
        let request = as_person(Request::post("/api/v1/themes?confirmed=true"), &cookie)
            .body(Body::from(bytes))
            .unwrap();
        let (status, _, body) = send(&api.router, request).await;
        assert!(status.is_success(), "{body}");
    }

    /// Runs `action` and asserts the folders were brought in line afterwards.
    async fn resyncs(api: &TestApi, what: &str, action: Value) -> Value {
        let before = api.slots.calls();
        let method = action["method"].as_str().unwrap();
        let path = action["path"].as_str().unwrap();
        let body = action.get("body").cloned();
        let (status, json) = call_at(api, method, path, body).await;
        assert!(status < 300, "{what}: {status} {json}");
        assert!(api.slots.calls() > before, "{what} did not re-sync");
        json
    }

    async fn call_at(api: &TestApi, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let request = bearer(Request::builder().method(method).uri(path), &api.token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    #[tokio::test]
    async fn every_change_that_affects_the_folders_brings_them_in_line() {
        let api = test_api().await;
        let created = resyncs(
            &api,
            "create account",
            json!({ "method": "POST", "path": "/api/v1/accounts", "body": { "name": "Max" } }),
        )
        .await;
        resyncs(
            &api,
            "create and login",
            json!({ "method": "POST", "path": "/api/v1/accounts/login",
                    "body": { "email": "me@example.com" } }),
        )
        .await;
        let path = format!(
            "/api/v1/accounts/{}?version={}",
            created["id"].as_str().unwrap(),
            created["version"]
        );
        resyncs(
            &api,
            "delete account",
            json!({ "method": "DELETE", "path": path }),
        )
        .await;

        upload(&api).await;
        resyncs(
            &api,
            "choose theme",
            json!({ "method": "PUT", "path": "/api/v1/settings/theme",
                    "body": { "theme": "night-blue" } }),
        )
        .await;
        let saved = resyncs(
            &api,
            "save settings",
            json!({ "method": "PUT", "path": "/api/v1/themes/night-blue/settings",
                    "body": { "values": { "dim": false }, "version": 0 } }),
        )
        .await;
        let path = format!(
            "/api/v1/themes/night-blue/settings?version={}",
            saved["version"]
        );
        resyncs(
            &api,
            "reset settings",
            json!({ "method": "DELETE", "path": path }),
        )
        .await;
        let (_, listed) = call_at(&api, "GET", "/api/v1/themes", None).await;
        let path = format!(
            "/api/v1/themes/night-blue?version={}",
            listed["items"][0]["version"]
        );
        resyncs(
            &api,
            "remove theme",
            json!({ "method": "DELETE", "path": path }),
        )
        .await;
        assert!(
            api.slots
                .recorded()
                .iter()
                .any(|call| call == "clear /default")
        );
    }
}
