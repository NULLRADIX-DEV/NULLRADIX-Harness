//! The switch that starts NULLRADIX when the person logs in.

use axum::Json;
use axum::extract::State;
use nullradix_app::autostart::AutostartStatus;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, ApiJson, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AutostartDto {
    /// Whether this host can start NULLRADIX at login; a container cannot.
    pub available: bool,
    /// Whether the login entry exists now.
    pub enabled: bool,
}

impl From<AutostartStatus> for AutostartDto {
    fn from(status: AutostartStatus) -> Self {
        Self {
            available: status.available,
            enabled: status.enabled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetAutostart {
    /// Start NULLRADIX at login, or stop doing so.
    pub enabled: bool,
}

#[utoipa::path(
    get,
    path = "/settings/autostart",
    tag = "settings",
    responses((status = 200, body = AutostartDto), (status = 401, body = Problem), (status = 500, description = "`autostart-failed`", body = Problem))
)]
pub(crate) async fn autostart(
    State(state): State<ApiState>,
) -> Result<Json<AutostartDto>, ApiError> {
    Ok(Json(state.autostart.status().await?.into()))
}

#[utoipa::path(
    put,
    path = "/settings/autostart",
    tag = "settings",
    request_body = SetAutostart,
    responses(
        (status = 200, description = "The login entry now matches the switch", body = AutostartDto),
        (status = 401, body = Problem),
        (status = 422, body = Problem),
        (status = 500, description = "`autostart-failed`", body = Problem),
        (status = 503, description = "`autostart-unavailable`: this host has no login", body = Problem),
    )
)]
pub(crate) async fn set_autostart(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<SetAutostart>,
) -> Result<Json<AutostartDto>, ApiError> {
    Ok(Json(state.autostart.set(input.enabled).await?.into()))
}

#[cfg(test)]
pub(crate) mod fake {
    use async_trait::async_trait;
    use nullradix_app::autostart::Autostart;
    use parking_lot::Mutex;

    /// A login entry kept in memory.
    #[derive(Default)]
    pub(crate) struct Entry(Mutex<bool>);

    #[async_trait]
    impl Autostart for Entry {
        fn available(&self) -> bool {
            true
        }

        async fn is_enabled(&self) -> Result<bool, String> {
            Ok(*self.0.lock())
        }

        async fn set(&self, enabled: bool) -> Result<(), String> {
            *self.0.lock() = enabled;
            Ok(())
        }
    }
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
                .uri("/api/v1/settings/autostart"),
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
        assert_eq!(off, json!({ "available": true, "enabled": false }));
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
            local(Request::get("/api/v1/settings/autostart")),
            local(Request::put("/api/v1/settings/autostart")),
        ] {
            let (status, _, _) = send(&api.router, request.body(Body::empty()).unwrap()).await;
            assert_eq!(status, 401);
        }
    }
}
