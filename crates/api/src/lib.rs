//! HTTP API served by the daemon.

mod health;
mod problem;

use std::future::Future;
use std::io;

use axum::Router;
use axum::http::Request;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub use problem::{ApiError, Problem};

/// Major version of the HTTP contract; clients refuse to talk to a different one.
pub const API_VERSION: u32 = 1;

#[derive(OpenApi)]
#[openapi(info(title = "NULLRADIX API"))]
struct ApiDoc;

fn routes() -> OpenApiRouter {
    OpenApiRouter::with_openapi(ApiDoc::openapi()).routes(routes!(health::health))
}

pub fn router() -> Router {
    let (router, _) = routes().split_for_parts();
    router.layer(
        ServiceBuilder::new()
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .layer(TraceLayer::new_for_http().make_span_with(request_span))
            .layer(PropagateRequestIdLayer::x_request_id()),
    )
}

fn request_span<B>(request: &Request<B>) -> tracing::Span {
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    tracing::info_span!(
        "request",
        method = %request.method(),
        path = %request.uri().path(),
        request_id,
    )
}

/// OpenAPI document of the whole API.
pub fn openapi_json() -> Result<String, serde_json::Error> {
    routes().split_for_parts().1.to_pretty_json()
}

pub async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_document_lists_health() {
        let json = openapi_json().unwrap();
        assert!(json.contains("\"/health\""));
        assert!(json.contains("NULLRADIX API"));
    }
}
