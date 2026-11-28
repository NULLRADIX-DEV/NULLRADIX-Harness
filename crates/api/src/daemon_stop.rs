use axum::extract::{Extension, State};
use axum::http::StatusCode;
use nullradix_app::auth::{Principal, decision_needs_person};

use crate::auth::{AgentContext, refuse_agent, require_token};
use crate::{ApiError, ApiState, Problem};

#[utoipa::path(
    post,
    path = "/daemon/stop",
    tag = "system",
    security(("bearer" = [])),
    responses(
        (status = 202, description = "The daemon ends open requests and exits; its runs go on"),
        (status = 401, body = Problem),
        (status = 403, description = "Sessions cannot stop the daemon; neither can runs of agents", body = Problem),
    )
)]
pub(crate) async fn stop_daemon(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Extension(context): Extension<AgentContext>,
) -> Result<StatusCode, ApiError> {
    require_token(&principal, "stop the daemon")?;
    refuse_agent(context, || decision_needs_person("stopping the daemon"))?;
    state.shutdown.cancel();
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;

    use super::*;
    use crate::tests::{
        as_person, bearer, in_run, in_session, local, open_session, send, test_api,
    };

    #[tokio::test]
    async fn a_token_outside_runs_stops_the_daemon() {
        let api = test_api().await;
        let request = bearer(Request::post("/api/v1/daemon/stop"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, _) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(api.shutdown.is_cancelled());
    }

    #[tokio::test]
    async fn sessions_agents_and_strangers_cannot_stop_it() {
        let api = test_api().await;
        let cookie = open_session(&api).await;
        let request = as_person(Request::post("/api/v1/daemon/stop"), &cookie)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(
            (status, json["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("token-required"))
        );
        for mark in [in_run, in_session] {
            let request = mark(bearer(Request::post("/api/v1/daemon/stop"), &api.token))
                .body(Body::empty())
                .unwrap();
            let (status, _, json) = send(&api.router, request).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(json["code"], "decision-needs-person");
        }
        let request = local(Request::post("/api/v1/daemon/stop"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&api.router, request).await.0, StatusCode::UNAUTHORIZED);
        assert!(!api.shutdown.is_cancelled());
    }
}
