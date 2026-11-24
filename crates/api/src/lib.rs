//! HTTP API served by the daemon.

mod auth;
mod events;
mod extract;
mod health;
mod host;
mod problem;
mod projects;
mod web;

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use nullradix_app::auth::AuthService;
use nullradix_app::projects::ProjectService;
use nullradix_app::{AppError, ChangeFeed, ChangeSignal};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceBuilder;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub use auth::{Handoff, Me, PrincipalKind, SESSION_COOKIE};
pub use events::{ChangeDto, ChangeOpDto, EntityDto, StreamPosition};
pub use extract::{ApiJson, ApiPath, ApiQuery};
pub use health::Health;
pub use host::AllowedHosts;
pub use problem::{ApiError, Problem};
pub use projects::{ProjectDto, ProjectList};

/// Major version of the HTTP contract; clients refuse to talk to a different one.
pub const API_VERSION: u32 = 1;

/// Prefix of every API route; the rest of the URL space belongs to the web UI.
pub const API_PREFIX: &str = "/api/v1";

#[derive(OpenApi)]
#[openapi(
    info(title = "NULLRADIX API"),
    components(schemas(Problem, ChangeDto, StreamPosition)),
    modifiers(&SecuritySchemes),
    security(("bearer" = []), ("session" = []))
)]
struct ApiDoc;

struct SecuritySchemes;

impl Modify for SecuritySchemes {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(SESSION_COOKIE))),
        );
    }
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState").finish_non_exhaustive()
    }
}

/// Everything the handlers need, built by the composition root.
#[derive(Clone)]
pub struct ApiState {
    pub auth: Arc<AuthService>,
    pub projects: Arc<ProjectService>,
    pub changes: Arc<dyn ChangeFeed>,
    pub signal: ChangeSignal,
    /// Cancelled when the daemon stops; long-lived responses end with it.
    pub shutdown: CancellationToken,
    pub allowed_hosts: AllowedHosts,
    pub secure_cookies: bool,
    /// Built web UI to serve next to the API, if any.
    pub web_dir: Option<PathBuf>,
}

fn routes() -> OpenApiRouter<ApiState> {
    let v1 = OpenApiRouter::new()
        .routes(routes!(health::health))
        .routes(routes!(auth::create_handoff))
        .routes(routes!(auth::redeem_handoff))
        .routes(routes!(auth::logout))
        .routes(routes!(auth::me))
        .routes(routes!(events::events))
        .routes(routes!(projects::list_projects, projects::create_project))
        .routes(routes!(
            projects::get_project,
            projects::rename_project,
            projects::delete_project
        ));
    let mut doc = ApiDoc::openapi();
    doc.info.version = API_VERSION.to_string();
    OpenApiRouter::with_openapi(doc).nest(API_PREFIX, v1)
}

pub fn router(state: ApiState) -> Router {
    let (router, _) = routes().split_for_parts();
    router
        .fallback(fallback)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::authenticate,
        ))
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
                .layer(TraceLayer::new_for_http().make_span_with(request_span))
                .layer(PropagateRequestIdLayer::x_request_id())
                .layer(security_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
                .layer(security_header(header::X_FRAME_OPTIONS, "DENY"))
                .layer(security_header(header::REFERRER_POLICY, "no-referrer"))
                .layer(security_header(
                    header::CONTENT_SECURITY_POLICY,
                    "frame-ancestors 'none'; object-src 'none'; base-uri 'self'",
                ))
                .layer(middleware::from_fn(web::no_store_for_api))
                .layer(middleware::from_fn_with_state(
                    state.allowed_hosts.clone(),
                    host::check_host,
                )),
        )
        .with_state(state)
}

fn security_header(
    name: header::HeaderName,
    value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
}

async fn fallback(State(state): State<ApiState>, request: axum::extract::Request) -> Response {
    match &state.web_dir {
        Some(dir) if !web::is_api_path(request.uri().path()) => web::serve(dir, request).await,
        _ => route_not_found().into_response(),
    }
}

fn route_not_found() -> ApiError {
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
pub(crate) mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{HeaderValue, header};
    use nullradix_app::SystemClock;
    use nullradix_app::UuidV7Generator;
    use nullradix_app::auth::Secret;
    use nullradix_store::Store;
    use tower::ServiceExt;

    use super::*;

    pub(crate) struct TestApi {
        pub(crate) router: Router,
        pub(crate) token: Secret,
        pub(crate) auth: Arc<AuthService>,
        pub(crate) projects: Arc<ProjectService>,
        pub(crate) changes: Arc<dyn ChangeFeed>,
        pub(crate) shutdown: CancellationToken,
    }

    pub(crate) async fn test_api() -> TestApi {
        test_api_with_web(None).await
    }

    pub(crate) async fn test_api_with_web(web_dir: Option<PathBuf>) -> TestApi {
        let store = Arc::new(Store::in_memory().await.unwrap());
        let auth = Arc::new(AuthService::new(
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
        ));
        let token = auth.local_token(None).await.unwrap();
        let signal = ChangeSignal::new();
        let shutdown = CancellationToken::new();
        let projects = Arc::new(ProjectService::new(
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ));
        let changes: Arc<dyn ChangeFeed> = store.clone();
        let router = router(ApiState {
            auth: auth.clone(),
            projects: projects.clone(),
            changes: changes.clone(),
            signal,
            shutdown: shutdown.clone(),
            allowed_hosts: AllowedHosts::loopback_and([]),
            secure_cookies: false,
            web_dir,
        });
        TestApi {
            router,
            token,
            auth,
            projects,
            changes,
            shutdown,
        }
    }

    pub(crate) fn local(request: axum::http::request::Builder) -> axum::http::request::Builder {
        request.header(header::HOST, "127.0.0.1:4711")
    }

    pub(crate) fn bearer(
        request: axum::http::request::Builder,
        token: &Secret,
    ) -> axum::http::request::Builder {
        local(request).header(header::AUTHORIZATION, format!("Bearer {}", token.expose()))
    }

    pub(crate) async fn send(
        router: &Router,
        request: Request<Body>,
    ) -> (StatusCode, Option<HeaderValue>, serde_json::Value) {
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json = if body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        (status, content_type, json)
    }

    #[tokio::test]
    async fn unknown_routes_answer_with_problem_details() {
        let api = test_api().await;
        let request = bearer(Request::get("/api/v1/nope"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, content_type, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(content_type.unwrap(), "application/problem+json");
        assert_eq!(json["code"], "route-not-found");
    }

    #[tokio::test]
    async fn wrong_methods_answer_with_problem_details() {
        let api = test_api().await;
        let request = local(Request::post("/api/v1/health"))
            .body(Body::empty())
            .unwrap();
        let (status, content_type, _) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(content_type.unwrap(), "application/problem+json");
    }

    #[tokio::test]
    async fn refuses_foreign_host_names() {
        let api = test_api().await;
        let request = Request::get("/api/v1/health")
            .header(header::HOST, "attacker.example.com:4711")
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["code"], "host-not-allowed");
    }

    #[test]
    fn openapi_document_lists_routes_and_security() {
        let json = openapi_json().unwrap();
        assert!(json.contains("\"/api/v1/health\""));
        assert!(json.contains("\"/api/v1/auth/handoffs\""));
        assert!(json.contains("NULLRADIX API"));
        assert!(json.contains("\"Problem\""));
        assert!(json.contains("\"nullradix_session\""));
    }

    #[test]
    fn openapi_version_is_the_api_version() {
        let json: serde_json::Value = serde_json::from_str(&openapi_json().unwrap()).unwrap();
        assert_eq!(json["info"]["version"], API_VERSION.to_string());
    }
}
