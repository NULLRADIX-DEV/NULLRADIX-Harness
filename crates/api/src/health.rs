use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::API_VERSION;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub status: String,
    pub version: String,
    pub api_version: u32,
}

#[utoipa::path(
    get,
    path = "/health",
    tag = "system",
    security(()),
    responses((status = 200, description = "The daemon is running", body = Health))
)]
pub(crate) async fn health() -> Json<Health> {
    Json(Health {
        status: String::from("ok"),
        version: String::from(env!("CARGO_PKG_VERSION")),
        api_version: API_VERSION,
    })
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};

    use crate::tests::{local, send, test_api};

    #[tokio::test]
    async fn reports_status_and_versions_without_credentials() {
        let api = test_api().await;
        let request = local(Request::get("/api/v1/health"))
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "ok");
        assert_eq!(json["apiVersion"], 1);
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    }
}
