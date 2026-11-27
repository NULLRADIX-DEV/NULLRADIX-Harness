use axum::Json;
use axum::extract::{Extension, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use nullradix_app::AppError;
use nullradix_app::auth::{Principal, Secret};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{API_PREFIX, ApiError, ApiQuery, ApiState, Problem};

pub const SESSION_COOKIE: &str = "nullradix_session";

const PUBLIC_ROUTES: &[&str] = &["/health", "/auth/handoff"];

enum Credentials {
    Missing,
    Malformed,
    Bearer(Secret),
    Session(Secret),
}

fn credentials(headers: &HeaderMap) -> Credentials {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        return value
            .to_str()
            .ok()
            .and_then(bearer_secret)
            .and_then(Secret::parse)
            .map_or(Credentials::Malformed, Credentials::Bearer);
    }
    match CookieJar::from_headers(headers).get(SESSION_COOKIE) {
        Some(cookie) => {
            Secret::parse(cookie.value()).map_or(Credentials::Malformed, Credentials::Session)
        }
        None => Credentials::Missing,
    }
}

fn bearer_secret(value: &str) -> Option<&str> {
    let (scheme, secret) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(secret.trim())
}

/// Cookie-authenticated writes must come from a page of this very origin, port included.
fn same_origin(headers: &HeaderMap) -> bool {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|origin| origin.to_str().ok())
        .and_then(|origin| {
            origin
                .strip_prefix("http://")
                .or_else(|| origin.strip_prefix("https://"))
        });
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok());
    matches!((origin, host), (Some(origin), Some(host)) if origin.eq_ignore_ascii_case(host))
}

/// Refuses browser sessions where only local token holders may act.
pub(crate) fn require_token(principal: &Principal, action: &str) -> Result<(), ApiError> {
    if matches!(principal, Principal::Token { .. }) {
        Ok(())
    } else {
        Err(
            AppError::forbidden("token-required", format!("only token holders can {action}"))
                .into(),
        )
    }
}

/// Header the CLI sends inside a dispatcher run, from `NULLRADIX_RUN`.
pub const RUN_HEADER: &str = "x-nullradix-run";
/// Header the CLI sends inside a Claude session, from `NULLRADIX_SESSION`.
pub const SESSION_HEADER: &str = "x-nullradix-session";

/// Whether the request says it comes from a run or a session of an agent; self-reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentContext(bool);

impl AgentContext {
    fn of(headers: &HeaderMap) -> Self {
        Self(headers.contains_key(RUN_HEADER) || headers.contains_key(SESSION_HEADER))
    }

    pub(crate) const fn is_agent(self) -> bool {
        self.0
    }
}

/// Whether the person in the app sent the request: a browser session that names no agent.
pub(crate) fn is_person(principal: &Principal, context: AgentContext) -> bool {
    matches!(principal, Principal::Session { .. }) && !context.is_agent()
}

/// Lets only the person in the app through; tokens and requests of agents get `refusal`.
pub(crate) fn require_person(
    principal: &Principal,
    context: AgentContext,
    refusal: impl FnOnce() -> AppError,
) -> Result<(), ApiError> {
    if is_person(principal, context) {
        Ok(())
    } else {
        Err(refusal().into())
    }
}

/// Lets a person through in the app or at a terminal, but no run or session of an agent.
pub(crate) fn refuse_agent(
    context: AgentContext,
    refusal: impl FnOnce() -> AppError,
) -> Result<(), ApiError> {
    if context.is_agent() {
        Err(refusal().into())
    } else {
        Ok(())
    }
}

pub(crate) async fn authenticate(
    State(state): State<ApiState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(route) = request.uri().path().strip_prefix(API_PREFIX) else {
        return next.run(request).await;
    };
    if PUBLIC_ROUTES.contains(&route) {
        return next.run(request).await;
    }
    let principal = match credentials(request.headers()) {
        Credentials::Bearer(secret) => state.auth.authenticate_token(&secret).await,
        Credentials::Session(secret) => {
            let safe = matches!(
                *request.method(),
                Method::GET | Method::HEAD | Method::OPTIONS
            );
            if safe || same_origin(request.headers()) {
                state.auth.authenticate_session(&secret).await
            } else {
                Err(AppError::forbidden(
                    "cross-site-request",
                    "writes with a session need a same-origin request",
                ))
            }
        }
        Credentials::Malformed => Err(AppError::unauthorized(
            "credentials-invalid",
            "the credentials are malformed",
        )),
        Credentials::Missing => Err(AppError::unauthorized(
            "credentials-missing",
            "this route needs a token or a session",
        )),
    };
    match principal {
        Ok(principal) => {
            let context = AgentContext::of(request.headers());
            request.extensions_mut().insert(principal);
            request.extensions_mut().insert(context);
            next.run(request).await
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

/// One-time code that opens a browser session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub code: String,
    pub path: String,
}

#[utoipa::path(
    post,
    path = "/auth/handoffs",
    tag = "auth",
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Code to open a browser session", body = Handoff),
        (status = 401, body = Problem),
        (status = 403, description = "Sessions cannot create handoffs", body = Problem),
    )
)]
pub(crate) async fn create_handoff(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
) -> Result<(StatusCode, Json<Handoff>), ApiError> {
    require_token(&principal, "open browser sessions")?;
    let code = state.auth.create_handoff()?;
    let handoff = Handoff {
        path: format!("{API_PREFIX}/auth/handoff?code={}", code.expose()),
        code: code.expose().to_owned(),
    };
    Ok((StatusCode::CREATED, Json(handoff)))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct HandoffQuery {
    code: String,
}

#[utoipa::path(
    get,
    path = "/auth/handoff",
    tag = "auth",
    security(()),
    params(HandoffQuery),
    responses(
        (status = 303, description = "Sets the session cookie and opens the web UI"),
        (status = 401, body = Problem),
    )
)]
pub(crate) async fn redeem_handoff(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<HandoffQuery>,
    jar: CookieJar,
) -> Result<(CookieJar, Redirect), ApiError> {
    let code = Secret::parse(&query.code).ok_or_else(|| {
        AppError::unauthorized("handoff-invalid", "the handoff code is unknown or expired")
    })?;
    let session = state.auth.redeem_handoff(&code).await?;
    if let Some(previous) = jar
        .get(SESSION_COOKIE)
        .and_then(|cookie| Secret::parse(cookie.value()))
    {
        state.auth.end_session(&previous).await?;
    }
    let cookie = Cookie::build((SESSION_COOKIE, session.expose().to_owned()))
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(state.secure_cookies)
        .path("/");
    Ok((jar.add(cookie), Redirect::to("/")))
}

#[utoipa::path(
    post,
    path = "/auth/logout",
    tag = "auth",
    security(("session" = [])),
    responses((status = 204, description = "The session is closed"), (status = 401, body = Problem))
)]
pub(crate) async fn logout(
    State(state): State<ApiState>,
    jar: CookieJar,
) -> Result<(CookieJar, StatusCode), ApiError> {
    if let Some(secret) = jar
        .get(SESSION_COOKIE)
        .and_then(|cookie| Secret::parse(cookie.value()))
    {
        state.auth.end_session(&secret).await?;
    }
    let removal = Cookie::build(SESSION_COOKIE).path("/");
    Ok((jar.remove(removal), StatusCode::NO_CONTENT))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PrincipalKind {
    Token,
    Session,
}

/// The caller as the daemon sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub kind: PrincipalKind,
}

#[utoipa::path(
    get,
    path = "/auth/me",
    tag = "auth",
    responses((status = 200, body = Me), (status = 401, body = Problem))
)]
pub(crate) async fn me(Extension(principal): Extension<Principal>) -> Json<Me> {
    let kind = match principal {
        Principal::Token { .. } => PrincipalKind::Token,
        Principal::Session { .. } => PrincipalKind::Session,
    };
    Json(Me { kind })
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;

    use super::*;
    use crate::tests::{bearer, local, open_session, send, test_api};

    #[tokio::test]
    async fn protected_routes_need_credentials() {
        let api = test_api().await;
        let request = local(Request::get("/api/v1/auth/me"))
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["code"], "credentials-missing");

        let request = local(Request::get("/api/v1/auth/me"))
            .header(header::AUTHORIZATION, "Bearer nope")
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["code"], "credentials-invalid");
    }

    #[tokio::test]
    async fn tokens_identify_the_caller() {
        let api = test_api().await;
        let request = bearer(Request::get("/api/v1/auth/me"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["kind"], "token");
    }

    #[tokio::test]
    async fn a_handoff_turns_into_a_cookie_session() {
        let api = test_api().await;
        let request = bearer(Request::post("/api/v1/auth/handoffs"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::CREATED);
        assert!(
            json["path"]
                .as_str()
                .unwrap()
                .starts_with("/api/v1/auth/handoff?code=")
        );

        let cookie = open_session(&api).await;
        let request = local(Request::get("/api/v1/auth/me"))
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["kind"], "session");
    }

    #[tokio::test]
    async fn sessions_cannot_create_handoffs() {
        let api = test_api().await;
        let cookie = open_session(&api).await;
        let request = local(Request::post("/api/v1/auth/handoffs"))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, "http://127.0.0.1:4711")
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["code"], "token-required");
    }

    #[tokio::test]
    async fn session_writes_need_a_same_origin_request() {
        let api = test_api().await;
        let cookie = open_session(&api).await;
        for origin in [
            None,
            Some("http://attacker.example.com"),
            Some("http://127.0.0.1:9999"),
            Some("http://localhost:4711"),
        ] {
            let mut request =
                local(Request::post("/api/v1/auth/logout")).header(header::COOKIE, &cookie);
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            let (status, _, json) = send(&api.router, request.body(Body::empty()).unwrap()).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(json["code"], "cross-site-request");
        }
    }

    #[tokio::test]
    async fn logout_ends_the_session() {
        let api = test_api().await;
        let cookie = open_session(&api).await;
        let request = local(Request::post("/api/v1/auth/logout"))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, "http://127.0.0.1:4711")
            .body(Body::empty())
            .unwrap();
        let (status, _, _) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let request = local(Request::get("/api/v1/auth/me"))
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["code"], "session-invalid");
    }

    #[tokio::test]
    async fn the_bearer_scheme_is_case_insensitive() {
        let api = test_api().await;
        let request = local(Request::get("/api/v1/auth/me"))
            .header(
                header::AUTHORIZATION,
                format!("bearer {}", api.token.expose()),
            )
            .body(Body::empty())
            .unwrap();
        let (status, _, _) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn path_tricks_do_not_skip_authentication() {
        let api = test_api().await;
        for path in [
            "/api/v1//auth/me",
            "/api/v1/auth/me/",
            "/api/v1/../v1/auth/me",
        ] {
            let request = local(Request::get(path)).body(Body::empty()).unwrap();
            let (status, _, _) = send(&api.router, request).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        }
    }

    #[tokio::test]
    async fn bad_handoff_codes_are_refused() {
        let api = test_api().await;
        for path in ["/api/v1/auth/handoff?code=nope", "/api/v1/auth/handoff"] {
            let request = local(Request::get(path)).body(Body::empty()).unwrap();
            let (status, _, _) = send(&api.router, request).await;
            assert!(
                status == StatusCode::UNAUTHORIZED || status == StatusCode::UNPROCESSABLE_ENTITY
            );
        }
    }
}
