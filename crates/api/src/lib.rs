//! HTTP API served by the daemon.

mod health;
mod host;
mod json;
mod problem;

use std::future::Future;
use std::io;

use axum::Router;
use axum::http::{Request, StatusCode};
use axum::middleware;
use axum::response::Response;
use nullradix_app::AppError;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub use health::Health;
pub use host::AllowedHosts;
pub use json::ApiJson;
pub use problem::{ApiError, Problem};

/// Major version of the HTTP contract; clients refuse to talk to a different one.
pub const API_VERSION: u32 = 1;

#[derive(OpenApi)]
#[openapi(info(title = "NULLRADIX API"), components(schemas(Problem)))]
struct ApiDoc;

/// Settings the router needs from the composition root.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub allowed_hosts: AllowedHosts,
}

/// Prefix of every API route; the rest of the URL space belongs to the web UI.
pub const API_PREFIX: &str = "/api/v1";

fn routes() -> OpenApiRouter {
    let v1 = OpenApiRouter::new().routes(routes!(health::health));
    OpenApiRouter::with_openapi(ApiDoc::openapi()).nest(API_PREFIX, v1)
}

pub fn router(config: &ApiConfig) -> Router {
    let (router, _) = routes().split_for_parts();
    router
        .fallback(route_not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
                .layer(TraceLayer::new_for_http().make_span_with(request_span))
                .layer(PropagateRequestIdLayer::x_request_id())
                .layer(middleware::from_fn_with_state(
                    config.allowed_hosts.clone(),
                    host::check_host,
                )),
        )
}

async fn route_not_found() -> ApiError {
    AppError::not_found("route-not-found", "no such route").into()
}

async fn method_not_allowed() -> Response {
    problem::response(
        StatusCode::METHOD_NOT_ALLOWED,
        "method-not-allowed",
        "method not allowed on this route",
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
    use axum::body::Body;
    use axum::http::header;
    use tower::ServiceExt;

    use super::*;

    pub(crate) fn test_router() -> Router {
        router(&ApiConfig {
            allowed_hosts: AllowedHosts::loopback_and([]),
        })
    }

    pub(crate) fn local(request: axum::http::request::Builder) -> axum::http::request::Builder {
        request.header(header::HOST, "127.0.0.1:4711")
    }

    async fn call(request: Request<Body>) -> (StatusCode, String) {
        let response = test_router().oneshot(request).await.unwrap();
        let content_type = response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .to_owned();
        (response.status(), content_type)
    }

    #[tokio::test]
    async fn unknown_routes_answer_with_problem_details() {
        let request = local(Request::get("/api/v1/nope"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            call(request).await,
            (
                StatusCode::NOT_FOUND,
                String::from("application/problem+json")
            )
        );
    }

    #[tokio::test]
    async fn wrong_methods_answer_with_problem_details() {
        let request = local(Request::post("/api/v1/health"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            call(request).await,
            (
                StatusCode::METHOD_NOT_ALLOWED,
                String::from("application/problem+json")
            )
        );
    }

    #[tokio::test]
    async fn refuses_foreign_host_names() {
        let request = Request::get("/api/v1/health")
            .header(header::HOST, "attacker.example.com:4711")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            call(request).await,
            (
                StatusCode::FORBIDDEN,
                String::from("application/problem+json")
            )
        );
    }

    #[test]
    fn openapi_document_lists_health() {
        let json = openapi_json().unwrap();
        assert!(json.contains("\"/api/v1/health\""));
        assert!(json.contains("NULLRADIX API"));
        assert!(json.contains("\"Problem\""));
    }
}
