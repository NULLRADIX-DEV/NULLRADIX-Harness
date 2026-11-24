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
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn reports_status_and_versions() {
        let request = crate::tests::local(Request::get("/api/v1/health"))
            .body(Body::empty())
            .unwrap();
        let response = crate::tests::test_router().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("x-request-id"));
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["apiVersion"], 1);
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    }
}
