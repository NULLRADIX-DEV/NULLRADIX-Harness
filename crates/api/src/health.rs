use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

use crate::API_VERSION;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Health {
    status: &'static str,
    version: &'static str,
    api_version: u32,
}

#[utoipa::path(
    get,
    path = "/health",
    tag = "system",
    responses((status = 200, description = "The daemon is running", body = Health))
)]
pub(crate) async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
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
        let response = crate::router()
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("x-request-id"));
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["apiVersion"], 1);
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    }
}
