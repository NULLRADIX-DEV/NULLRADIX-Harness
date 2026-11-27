use axum::Json;
use axum::extract::{Extension, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use nullradix_app::AppError;
use nullradix_app::auth::{CommandWriter, Principal, Secret, decision_needs_person};
use nullradix_domain::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{API_PREFIX, ApiError, ApiJson, ApiQuery, ApiState, Problem};

pub const SESSION_COOKIE: &str = "nullradix_session";
/// Header with the key of the tab; a session cookie alone authenticates nobody.
pub const KEY_HEADER: &str = "x-nullradix-key";

const PUBLIC_ROUTES: &[&str] = &["/health", "/auth/handoff"];

enum Credentials {
    Missing,
    Malformed,
    Bearer(Secret),
    Session { cookie: Secret, key: Option<Secret> },
}

/// The key of the tab: the header, or `?k=` on the event stream, which cannot set headers.
fn tab_key(request: &Request, route: &str) -> Option<Secret> {
    let header = request
        .headers()
        .get(KEY_HEADER)
        .and_then(|value| value.to_str().ok());
    if header.is_some() {
        return header.and_then(Secret::parse);
    }
    if route != "/events" || request.method() != Method::GET {
        return None;
    }
    request
        .uri()
        .query()?
        .split('&')
        .find_map(|pair| pair.strip_prefix("k="))
        .and_then(Secret::parse)
}

/// Files a page loads without being able to send the key: only a cookie, read only.
fn is_theme_asset(method: &Method, route: &str) -> bool {
    if !matches!(*method, Method::GET | Method::HEAD) {
        return false;
    }
    let Some(rest) = route.strip_prefix("/themes/") else {
        return false;
    };
    let mut parts = rest.splitn(3, '/');
    let (Some(id), Some(kind)) = (parts.next(), parts.next()) else {
        return false;
    };
    let tail = parts.next();
    !id.is_empty()
        && match kind {
            "files" => tail.is_some_and(|tail| !tail.is_empty()),
            "fetch" => tail.is_none(),
            _ => false,
        }
}

fn credentials(headers: &HeaderMap, key: Option<Secret>) -> Credentials {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        return value
            .to_str()
            .ok()
            .and_then(bearer_secret)
            .and_then(Secret::parse)
            .map_or(Credentials::Malformed, Credentials::Bearer);
    }
    match CookieJar::from_headers(headers).get(SESSION_COOKIE) {
        Some(cookie) => Secret::parse(cookie.value()).map_or(Credentials::Malformed, |cookie| {
            Credentials::Session { cookie, key }
        }),
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

/// Who asks to change fields that run commands.
pub(crate) fn command_writer(principal: &Principal, context: AgentContext) -> CommandWriter {
    match principal {
        Principal::Session { elevated_until, .. } if !context.is_agent() => CommandWriter::Person {
            elevated: elevated_until.is_some(),
        },
        _ => CommandWriter::Other,
    }
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
    let key = tab_key(&request, route);
    let principal = match credentials(request.headers(), key) {
        Credentials::Bearer(secret) => state.auth.authenticate_token(&secret).await,
        Credentials::Session { cookie, key } => {
            let safe = matches!(
                *request.method(),
                Method::GET | Method::HEAD | Method::OPTIONS
            );
            if is_theme_asset(request.method(), route) {
                state.auth.authenticate_asset_session(&cookie).await
            } else if safe || same_origin(request.headers()) {
                state.auth.authenticate_session(&cookie, key.as_ref()).await
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
        (status = 303, description = "Sets the session cookie and opens the web UI with the key of the tab in the fragment"),
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
    let secrets = state.auth.redeem_handoff(&code).await?;
    if let Some(previous) = jar
        .get(SESSION_COOKIE)
        .and_then(|cookie| Secret::parse(cookie.value()))
    {
        state.auth.end_session(&previous).await?;
    }
    let cookie = Cookie::build((SESSION_COOKIE, secrets.session.expose().to_owned()))
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(state.secure_cookies)
        .path("/");
    // the fragment never goes to a server; the page keeps the key per tab and removes it
    let target = format!("/#k={}", secrets.key.expose());
    Ok((jar.add(cookie), Redirect::to(&target)))
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
    /// Unix milliseconds until a session may change command fields; none when it may not.
    pub elevated_until: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/auth/me",
    tag = "auth",
    responses((status = 200, body = Me), (status = 401, body = Problem))
)]
pub(crate) async fn me(Extension(principal): Extension<Principal>) -> Json<Me> {
    let (kind, elevated_until) = match principal {
        Principal::Token { .. } => (PrincipalKind::Token, None),
        Principal::Session { elevated_until, .. } => (
            PrincipalKind::Session,
            elevated_until.map(Timestamp::as_millis),
        ),
    };
    Json(Me {
        kind,
        elevated_until,
    })
}

/// One-time code that confirms a session when the person gives it to the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepUpCode {
    pub code: String,
    /// Address of the page with the code in its fragment, relative to the daemon.
    pub path: String,
}

#[utoipa::path(
    post,
    path = "/auth/step-up-codes",
    tag = "auth",
    security(("bearer" = [])),
    responses(
        (status = 201, description = "Code that confirms a browser session for a few minutes", body = StepUpCode),
        (status = 401, body = Problem),
        (status = 403, description = "Sessions cannot create codes; neither can runs of agents", body = Problem),
    )
)]
pub(crate) async fn create_step_up_code(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Extension(context): Extension<AgentContext>,
) -> Result<(StatusCode, Json<StepUpCode>), ApiError> {
    require_token(&principal, "confirm changes")?;
    refuse_agent(context, || {
        decision_needs_person("confirming a change of commands")
    })?;
    let code = state.auth.create_step_up_code()?;
    let created = StepUpCode {
        path: format!("/#confirm={}", code.expose()),
        code: code.expose().to_owned(),
    };
    Ok((StatusCode::CREATED, Json(created)))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RedeemStepUp {
    pub code: String,
}

/// How long the session may change command fields now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Elevation {
    /// Unix milliseconds.
    pub elevated_until: i64,
}

#[utoipa::path(
    post,
    path = "/auth/step-up",
    tag = "auth",
    security(("session" = [])),
    request_body = RedeemStepUp,
    responses(
        (status = 200, description = "The session may change command fields for a few minutes", body = Elevation),
        (status = 401, description = "The code is unknown or expired (`step-up-code-invalid`)", body = Problem),
        (status = 403, description = "Only the person in the app can confirm", body = Problem),
    )
)]
pub(crate) async fn redeem_step_up(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Extension(context): Extension<AgentContext>,
    ApiJson(input): ApiJson<RedeemStepUp>,
) -> Result<Json<Elevation>, ApiError> {
    let Principal::Session { id, .. } = principal else {
        return Err(AppError::forbidden(
            "session-required",
            "only the person in the app can confirm",
        )
        .into());
    };
    refuse_agent(context, || {
        decision_needs_person("confirming a change of commands")
    })?;
    let code = Secret::parse(&input.code).ok_or_else(|| {
        AppError::unauthorized(
            "step-up-code-invalid",
            "the confirmation code is unknown or expired",
        )
    })?;
    let until = state.auth.redeem_step_up(id, &code).await?;
    Ok(Json(Elevation {
        elevated_until: until.as_millis(),
    }))
}

/// Whether changing the fields that run commands needs a confirmation first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepUpSetting {
    pub required: bool,
}

#[utoipa::path(
    get,
    path = "/settings/step-up",
    tag = "auth",
    responses((status = 200, body = StepUpSetting), (status = 401, body = Problem))
)]
pub(crate) async fn step_up_setting(
    State(state): State<ApiState>,
) -> Result<Json<StepUpSetting>, ApiError> {
    Ok(Json(StepUpSetting {
        required: state.auth.step_up_required().await?,
    }))
}

#[utoipa::path(
    put,
    path = "/settings/step-up",
    tag = "auth",
    request_body = StepUpSetting,
    responses(
        (status = 200, body = StepUpSetting),
        (status = 401, body = Problem),
        (status = 403, description = "Turning it off needs the person in the app with a fresh confirmation (`step-up-required`); a token may only turn it on (`command-change-needs-person`)", body = Problem),
    )
)]
pub(crate) async fn set_step_up_setting(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Extension(context): Extension<AgentContext>,
    ApiJson(input): ApiJson<StepUpSetting>,
) -> Result<Json<StepUpSetting>, ApiError> {
    if !input.required
        && let Some(refusal) = state
            .auth
            .command_refusal(command_writer(&principal, context))
            .await?
    {
        return Err(refusal.error(&["required"]).into());
    }
    if let Some(change) = state.auth.set_step_up_required(input.required).await? {
        state.signal.notify(&change);
    }
    Ok(Json(StepUpSetting {
        required: input.required,
    }))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use nullradix_app::auth::AuthRepository;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::tests::{
        TestApi, as_person, bearer, in_run, in_session, key_of, local, open_session, send,
        test_api, without_key,
    };

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

    fn get_with(path: &str, cookie: &str, key: Option<&str>) -> Request<Body> {
        let mut request = local(Request::get(path)).header(header::COOKIE, cookie);
        if let Some(key) = key {
            request = request.header(KEY_HEADER, key);
        }
        request.body(Body::empty()).unwrap()
    }

    async fn me_with(api: &TestApi, cookie: &str, key: Option<&str>) -> (StatusCode, Value) {
        let (status, _, json) = send(&api.router, get_with("/api/v1/auth/me", cookie, key)).await;
        (status, json)
    }

    #[tokio::test]
    async fn the_handoff_redirect_carries_the_key_in_the_fragment_only() {
        let api = test_api().await;
        let code = api.auth.create_handoff().unwrap();
        let request = local(Request::get(format!(
            "/api/v1/auth/handoff?code={}",
            code.expose()
        )))
        .body(Body::empty())
        .unwrap();
        let response = api.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response.headers()[header::LOCATION].to_str().unwrap();
        let key = location.strip_prefix("/#k=").unwrap();
        assert_eq!(key.len(), 64);
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(!cookie.contains(key));
    }

    #[tokio::test]
    async fn a_cookie_without_the_key_is_nobody() {
        let api = test_api().await;
        let cookie = open_session(&api).await;
        let (status, json) = me_with(&api, &without_key(&cookie), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["code"], "session-key-missing");
        let (status, json) = me_with(&api, &without_key(&cookie), Some(&key_of(&cookie))).await;
        assert_eq!((status, &json["kind"]), (StatusCode::OK, &json!("session")));
        // a malformed key is no key
        let (status, json) = me_with(&api, &without_key(&cookie), Some("nope")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["code"], "session-key-missing");
    }

    #[tokio::test]
    async fn a_wrong_key_and_the_key_of_another_session_are_refused() {
        let api = test_api().await;
        let mine = open_session(&api).await;
        let other = open_session(&api).await;
        for key in ["d".repeat(64), key_of(&other)] {
            let (status, json) = me_with(&api, &without_key(&mine), Some(&key)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(json["code"], "session-key-invalid");
        }
    }

    #[tokio::test]
    async fn writes_with_a_session_need_the_key_too() {
        let api = test_api().await;
        let cookie = open_session(&api).await;
        let request = local(Request::post("/api/v1/auth/logout"))
            .header(header::COOKIE, without_key(&cookie))
            .header(header::ORIGIN, "http://127.0.0.1:4711")
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["code"], "session-key-missing");
    }

    #[tokio::test]
    async fn the_query_key_works_on_the_event_stream_alone() {
        let api = test_api().await;
        let cookie = open_session(&api).await;
        let key = key_of(&cookie);
        let bare = without_key(&cookie);
        let request = get_with(&format!("/api/v1/events?k={key}"), &bare, None);
        let response = api.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let request = get_with("/api/v1/events", &bare, None);
        let response = api.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let request = get_with(&format!("/api/v1/auth/me?k={key}"), &bare, None);
        let response = api.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        // a bearer token never needs a key
        let request = bearer(Request::get("/api/v1/auth/me"), &api.token)
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&api.router, request).await.0, StatusCode::OK);
    }

    #[test]
    fn only_reads_of_theme_files_and_the_proxy_skip_the_key() {
        let get = Method::GET;
        for route in [
            "/themes/night/files/theme.css",
            "/themes/night/files/assets/a.png",
            "/themes/night/fetch",
        ] {
            assert!(is_theme_asset(&get, route), "{route}");
            assert!(!is_theme_asset(&Method::POST, route), "{route}");
            assert!(!is_theme_asset(&Method::DELETE, route), "{route}");
        }
        for route in [
            "/themes",
            "/themes/night",
            "/themes/night/code",
            "/themes/night/settings",
            "/themes/night/files",
            "/themes/night/files/",
            "/themes//files/a.css",
            "/themes/night/fetch/more",
            "/auth/me",
            "/dispatcher/config",
        ] {
            assert!(!is_theme_asset(&get, route), "{route}");
        }
    }

    #[tokio::test]
    async fn theme_files_need_only_the_cookie_but_nothing_else_does() {
        let api = test_api().await;
        let cookie = without_key(&open_session(&api).await);
        // no theme named so: not found, which is past the authentication
        let (status, _, _) = send(
            &api.router,
            get_with("/api/v1/themes/none/files/theme.css", &cookie, None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _, _) = send(
            &api.router,
            get_with("/api/v1/themes/none/settings", &cookie, None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = send(&api.router, get_with("/api/v1/themes", &cookie, None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    async fn post_json(
        api: &TestApi,
        path: &str,
        builder: impl FnOnce(axum::http::request::Builder) -> axum::http::request::Builder,
        body: &Value,
    ) -> (StatusCode, Value) {
        let request = builder(Request::post(path))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status, json)
    }

    async fn step_up_code(api: &TestApi) -> String {
        let request = bearer(Request::post("/api/v1/auth/step-up-codes"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::CREATED);
        let code = json["code"].as_str().unwrap().to_owned();
        assert_eq!(json["path"], format!("/#confirm={code}"));
        code
    }

    async fn confirm(api: &TestApi, cookie: &str) -> (StatusCode, Value) {
        let code = step_up_code(api).await;
        post_json(
            api,
            "/api/v1/auth/step-up",
            |request| as_person(request, cookie),
            &json!({ "code": code }),
        )
        .await
    }

    #[tokio::test]
    async fn only_tokens_outside_runs_create_step_up_codes() {
        let api = test_api().await;
        let cookie = open_session(&api).await;
        let request = as_person(Request::post("/api/v1/auth/step-up-codes"), &cookie)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(
            (status, &json["code"]),
            (StatusCode::FORBIDDEN, &json!("token-required"))
        );
        for mark in [in_run, in_session] {
            let request = mark(bearer(
                Request::post("/api/v1/auth/step-up-codes"),
                &api.token,
            ))
            .body(Body::empty())
            .unwrap();
            let (status, _, json) = send(&api.router, request).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(json["code"], "decision-needs-person");
        }
        let request = local(Request::post("/api/v1/auth/step-up-codes"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&api.router, request).await.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_code_confirms_the_session_that_redeems_it_and_only_once() {
        let api = test_api().await;
        let mine = open_session(&api).await;
        let other = open_session(&api).await;
        let (status, json) = me_with(&api, &mine, None).await;
        assert_eq!(
            (status, &json["elevatedUntil"]),
            (StatusCode::OK, &Value::Null)
        );
        let code = step_up_code(&api).await;
        let redeem = |cookie: &str, code: &str| {
            let cookie = cookie.to_owned();
            let body = json!({ "code": code });
            let api = &api;
            async move {
                post_json(
                    api,
                    "/api/v1/auth/step-up",
                    |request| as_person(request, &cookie),
                    &body,
                )
                .await
            }
        };
        let (status, json) = redeem(&mine, &code).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        let until = json["elevatedUntil"].as_i64().unwrap();
        let (_, me) = me_with(&api, &mine, None).await;
        assert_eq!(me["elevatedUntil"], until);
        let (_, me) = me_with(&api, &other, None).await;
        assert_eq!(me["elevatedUntil"], Value::Null);
        let (status, json) = redeem(&mine, &code).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["code"], "step-up-code-invalid");
        let (status, json) = redeem(&mine, "nope").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["code"], "step-up-code-invalid");
    }

    #[tokio::test]
    async fn tokens_and_agents_cannot_redeem_a_code() {
        let api = test_api().await;
        let code = step_up_code(&api).await;
        let body = json!({ "code": code });
        let (status, json) = post_json(
            &api,
            "/api/v1/auth/step-up",
            |request| bearer(request, &api.token),
            &body,
        )
        .await;
        assert_eq!(
            (status, &json["code"]),
            (StatusCode::FORBIDDEN, &json!("session-required"))
        );
        let cookie = open_session(&api).await;
        let (status, json) = post_json(
            &api,
            "/api/v1/auth/step-up",
            |request| in_run(as_person(request, &cookie)),
            &body,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["code"], "decision-needs-person");
        // the refused attempts did not use the code
        assert_eq!(confirm_with(&api, &cookie, &code).await, StatusCode::OK);
    }

    async fn confirm_with(api: &TestApi, cookie: &str, code: &str) -> StatusCode {
        post_json(
            api,
            "/api/v1/auth/step-up",
            |request| as_person(request, cookie),
            &json!({ "code": code }),
        )
        .await
        .0
    }

    async fn config_view(api: &TestApi) -> Value {
        let request = bearer(Request::get("/api/v1/dispatcher/config"), &api.token)
            .body(Body::empty())
            .unwrap();
        send(&api.router, request).await.2
    }

    /// The configuration with one command field changed, as the body of a save.
    fn with_field(view: &Value, field: &str) -> Value {
        let mut config = view["config"].clone();
        match field {
            "claude.command" => config["claude"]["command"] = json!("/opt/claude"),
            "claude.extraArgs" => config["claude"]["extraArgs"] = json!(["--x"]),
            "claude.permissionMode" => config["claude"]["permissionMode"] = json!("plan"),
            "claude.allowedTools" => config["claude"]["allowedTools"] = json!(["Read"]),
            "claude.gitBashPath" => config["claude"]["gitBashPath"] = json!("C:/bash.exe"),
            "worktreeSetup" => {
                config["worktreeSetup"] = json!([{ "cwd": ".", "run": "npm ci" }]);
            }
            other => panic!("{other}"),
        }
        json!({ "version": view["version"], "config": config })
    }

    const COMMAND_FIELDS: [&str; 6] = [
        "claude.command",
        "claude.extraArgs",
        "claude.permissionMode",
        "claude.allowedTools",
        "claude.gitBashPath",
        "worktreeSetup",
    ];

    async fn save(
        api: &TestApi,
        body: &Value,
        builder: impl FnOnce(axum::http::request::Builder) -> axum::http::request::Builder,
    ) -> (StatusCode, Value) {
        let request = builder(Request::put("/api/v1/dispatcher/config"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status, json)
    }

    #[tokio::test]
    async fn tokens_and_agents_never_change_a_command_field() {
        let api = test_api().await;
        let view = config_view(&api).await;
        let cookie = open_session(&api).await;
        confirm(&api, &cookie).await;
        for field in COMMAND_FIELDS {
            let body = with_field(&view, field);
            let (status, json) = save(&api, &body, |request| bearer(request, &api.token)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{field} {json}");
            assert_eq!(json["code"], "command-change-needs-person", "{field}");
            assert_eq!(json["errors"][0]["field"], field);
            // an agent in the confirmed browser session is no person either
            for mark in [in_run, in_session] {
                let (status, json) =
                    save(&api, &body, |request| mark(as_person(request, &cookie))).await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{field} {json}");
                assert_eq!(json["code"], "command-change-needs-person");
            }
        }
        assert_eq!(config_view(&api).await["version"], view["version"]);
    }

    #[tokio::test]
    async fn a_session_needs_a_confirmation_for_each_command_field() {
        let api = test_api().await;
        let view = config_view(&api).await;
        let cookie = open_session(&api).await;
        for field in COMMAND_FIELDS {
            let body = with_field(&view, field);
            let (status, json) = save(&api, &body, |request| as_person(request, &cookie)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{field} {json}");
            assert_eq!(json["code"], "step-up-required", "{field}");
            assert!(
                json["detail"]
                    .as_str()
                    .unwrap()
                    .contains("nullradix confirm")
            );
            assert_eq!(json["errors"][0]["field"], field);
        }
        assert_eq!(config_view(&api).await["version"], view["version"]);
        // other fields do not need it, and a save that changes nothing does not either
        let mut config = view["config"].clone();
        config["wip"] = json!(2);
        let body = json!({ "version": view["version"], "config": config });
        let (status, json) = save(&api, &body, |request| as_person(request, &cookie)).await;
        assert_eq!(status, StatusCode::OK, "{json}");
    }

    #[tokio::test]
    async fn a_confirmed_session_changes_command_fields_until_the_confirmation_ends() {
        let api = test_api().await;
        let cookie = open_session(&api).await;
        assert_eq!(confirm(&api, &cookie).await.0, StatusCode::OK);
        let mut view = config_view(&api).await;
        for field in COMMAND_FIELDS {
            let body = with_field(&view, field);
            let (status, json) = save(&api, &body, |request| as_person(request, &cookie)).await;
            assert_eq!(status, StatusCode::OK, "{field} {json}");
            view = config_view(&api).await;
        }
        // the confirmation ends: the session itself says when, so make it the past
        let secret = Secret::parse(cookie.split(';').next().unwrap().split('=').nth(1).unwrap());
        let stored = api
            .store
            .session_by_hash(&secret.unwrap().hash())
            .await
            .unwrap()
            .unwrap();
        api.store
            .elevate_session(stored.id, Timestamp::from_millis(1))
            .await
            .unwrap();
        let mut config = view["config"].clone();
        config["claude"]["command"] = json!("claude");
        let body = json!({ "version": view["version"], "config": config });
        let (status, json) = save(&api, &body, |request| as_person(request, &cookie)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
        assert_eq!(json["code"], "step-up-required");
    }

    async fn put_switch(
        api: &TestApi,
        required: bool,
        builder: impl FnOnce(axum::http::request::Builder) -> axum::http::request::Builder,
    ) -> (StatusCode, Value) {
        let request = builder(Request::put("/api/v1/settings/step-up"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "required": required }).to_string()))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status, json)
    }

    async fn switch(api: &TestApi) -> bool {
        let request = bearer(Request::get("/api/v1/settings/step-up"), &api.token)
            .body(Body::empty())
            .unwrap();
        send(&api.router, request).await.2["required"]
            .as_bool()
            .unwrap()
    }

    #[tokio::test]
    async fn turning_the_confirmation_off_needs_a_confirmation_and_a_token_may_only_turn_it_on() {
        let api = test_api().await;
        let cookie = open_session(&api).await;
        assert!(switch(&api).await);
        // a token and an agent in the session cannot turn it off, whatever the confirmation
        let (status, json) = put_switch(&api, false, |request| bearer(request, &api.token)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["code"], "command-change-needs-person");
        assert_eq!(json["errors"][0]["field"], "required");
        // the person without a confirmation cannot either
        let (status, json) = put_switch(&api, false, |request| as_person(request, &cookie)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["code"], "step-up-required");
        assert!(switch(&api).await);
        confirm(&api, &cookie).await;
        let (status, _) =
            put_switch(&api, false, |request| in_run(as_person(request, &cookie))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(switch(&api).await);
        let (status, json) = put_switch(&api, false, |request| as_person(request, &cookie)).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert!(!switch(&api).await);

        // off: the person changes command fields at once, a token still may not
        let fresh = open_session(&api).await;
        let view = config_view(&api).await;
        let body = with_field(&view, "claude.command");
        let (status, json) = save(&api, &body, |request| bearer(request, &api.token)).await;
        assert_eq!(
            (status, &json["code"]),
            (StatusCode::FORBIDDEN, &json!("command-change-needs-person"))
        );
        let (status, json) = save(&api, &body, |request| as_person(request, &fresh)).await;
        assert_eq!(status, StatusCode::OK, "{json}");

        // anyone can turn it back on, a token and an unconfirmed session included
        let (status, _) = put_switch(&api, true, |request| bearer(request, &api.token)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(switch(&api).await);
        let (status, _) = put_switch(&api, false, |request| bearer(request, &api.token)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let third = open_session(&api).await;
        confirm(&api, &third).await;
        put_switch(&api, false, |request| as_person(request, &third)).await;
        let (status, _) = put_switch(&api, true, |request| as_person(request, &fresh)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(switch(&api).await);
    }
}
