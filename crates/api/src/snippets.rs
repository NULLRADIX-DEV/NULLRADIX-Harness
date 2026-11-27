//! CSS snippets: small stylesheets on top of the active theme.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nullradix_app::AppError;
use nullradix_domain::Snippet;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{API_PREFIX, ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SnippetDto {
    /// Chosen by the person: 2 to 40 characters from a-z, 0-9 and -.
    pub id: String,
    pub name: String,
    pub css: String,
    pub enabled: bool,
    /// Position in the delivered stylesheet; later snippets win.
    pub order: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: i64,
}

impl From<&Snippet> for SnippetDto {
    fn from(snippet: &Snippet) -> Self {
        let record = snippet.record();
        Self {
            id: record.id.to_string(),
            name: record.name.clone(),
            css: record.css.clone(),
            enabled: record.enabled,
            order: record.order,
            created_at: record.created_at.as_millis(),
            updated_at: record.updated_at.as_millis(),
            version: record.version.value(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SnippetList {
    /// In the order they are delivered.
    pub items: Vec<SnippetDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateSnippet {
    pub id: String,
    pub name: String,
    /// At most 64 KiB, without `@import` and without urls other than `data:`.
    pub css: String,
    /// New snippets are on unless this says otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSnippet {
    /// Replaces the name; left out keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Replaces the style text; left out keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Switches the snippet on or off; left out keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Version the change is based on; a newer stored version fails with `version-conflict`.
    pub version: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct SnippetPath {
    /// Id of the snippet.
    id: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct DeleteSnippetQuery {
    /// Version the deletion is based on.
    version: i64,
}

#[utoipa::path(
    get,
    path = "/snippets",
    tag = "snippets",
    responses((status = 200, body = SnippetList), (status = 401, body = Problem))
)]
pub(crate) async fn list_snippets(
    State(state): State<ApiState>,
) -> Result<Json<SnippetList>, ApiError> {
    let snippets = state.snippets.list().await?;
    Ok(Json(SnippetList {
        items: snippets.iter().map(SnippetDto::from).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/snippets",
    tag = "snippets",
    request_body = CreateSnippet,
    responses(
        (status = 201, body = SnippetDto,
            headers(("Location" = String, description = "URL of the snippet"))),
        (status = 401, body = Problem),
        (status = 409, description = "The id is taken: `snippet-exists`", body = Problem),
        (status = 422, description = "Bad id, name or style text", body = Problem),
    )
)]
pub(crate) async fn create_snippet(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<CreateSnippet>,
) -> Result<(StatusCode, HeaderMap, Json<SnippetDto>), ApiError> {
    let snippet = state
        .snippets
        .create(&input.id, &input.name, &input.css, input.enabled)
        .await?;
    let dto = SnippetDto::from(&snippet);
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&format!("{API_PREFIX}/snippets/{}", dto.id)) {
        headers.insert(header::LOCATION, value);
    }
    Ok((StatusCode::CREATED, headers, Json(dto)))
}

#[utoipa::path(
    get,
    path = "/snippets/{id}",
    tag = "snippets",
    params(SnippetPath),
    responses(
        (status = 200, body = SnippetDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn get_snippet(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<SnippetPath>,
) -> Result<Json<SnippetDto>, ApiError> {
    let snippet = state.snippets.get(&path.id).await?;
    Ok(Json(SnippetDto::from(&snippet)))
}

#[utoipa::path(
    patch,
    path = "/snippets/{id}",
    tag = "snippets",
    params(SnippetPath),
    request_body = UpdateSnippet,
    responses(
        (status = 200, body = SnippetDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn update_snippet(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<SnippetPath>,
    ApiJson(input): ApiJson<UpdateSnippet>,
) -> Result<Json<SnippetDto>, ApiError> {
    let snippet = state
        .snippets
        .update(
            &path.id,
            input.name.as_deref(),
            input.css.as_deref(),
            input.enabled,
            AppError::client_version(input.version)?,
        )
        .await?;
    Ok(Json(SnippetDto::from(&snippet)))
}

#[utoipa::path(
    delete,
    path = "/snippets/{id}",
    tag = "snippets",
    params(SnippetPath, DeleteSnippetQuery),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn delete_snippet(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<SnippetPath>,
    ApiQuery(query): ApiQuery<DeleteSnippetQuery>,
) -> Result<StatusCode, ApiError> {
    state
        .snippets
        .delete(&path.id, AppError::client_version(query.version)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/snippets/active.css",
    tag = "snippets",
    responses(
        (status = 200, description = "The enabled snippets as one stylesheet", content_type = "text/css", body = String),
        (status = 401, body = Problem),
    )
)]
pub(crate) async fn active_snippets(State(state): State<ApiState>) -> Result<Response, ApiError> {
    let css = state.snippets.active_css().await?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            // the stylesheet changes with every edit, so the browser asks again
            (header::CACHE_CONTROL, "no-cache"),
        ],
        css,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use serde_json::{Value, json};

    use crate::snippets::SnippetList;
    use crate::tests::{TestApi, bearer, local, send, test_api};

    async fn call(api: &TestApi, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let request = bearer(Request::builder().method(method).uri(path), &api.token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    async fn stylesheet(api: &TestApi) -> (u16, Option<String>, Option<String>, String) {
        let request = bearer(Request::get("/api/v1/snippets/active.css"), &api.token)
            .body(Body::empty())
            .unwrap();
        let response = tower::ServiceExt::oneshot(api.router.clone(), request)
            .await
            .unwrap();
        let status = response.status().as_u16();
        let header_text = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let kind = header_text(header::CONTENT_TYPE);
        let csp = header_text(header::CONTENT_SECURITY_POLICY);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            kind,
            csp,
            String::from_utf8(bytes.to_vec()).unwrap(),
        )
    }

    #[allow(clippy::unnecessary_wraps, reason = "it is the body of a request")]
    fn create(id: &str, css: &str) -> Option<Value> {
        Some(json!({ "id": id, "name": format!("Name {id}"), "css": css }))
    }

    #[tokio::test]
    async fn a_snippet_is_created_listed_changed_and_deleted() {
        let api = test_api().await;
        let (status, empty) = call(&api, "GET", "/api/v1/snippets", None).await;
        assert_eq!((status, &empty), (200, &json!({ "items": [] })));

        let (status, created) = call(&api, "POST", "/api/v1/snippets", create("red", "a{}")).await;
        assert_eq!(status, 201, "{created}");
        assert_eq!(created["id"], "red");
        assert_eq!(created["enabled"], true);
        assert_eq!(created["order"], 1);
        assert_eq!(created["version"], 1);
        let (_, second) = call(
            &api,
            "POST",
            "/api/v1/snippets",
            Some(json!({ "id": "blue", "name": "Blue", "css": "b{}", "enabled": false })),
        )
        .await;
        assert_eq!(
            (&second["enabled"], &second["order"]),
            (&json!(false), &json!(2))
        );

        let (_, list) = call(&api, "GET", "/api/v1/snippets", None).await;
        let list: SnippetList = serde_json::from_value(list).unwrap();
        let ids: Vec<&str> = list.items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["red", "blue"]);

        let (status, patched) = call(
            &api,
            "PATCH",
            "/api/v1/snippets/red",
            Some(json!({ "css": "a{color:red}", "name": "Red", "version": 1 })),
        )
        .await;
        assert_eq!(
            (
                status,
                &patched["css"],
                &patched["name"],
                &patched["version"]
            ),
            (200, &json!("a{color:red}"), &json!("Red"), &json!(2))
        );
        let (status, problem) = call(
            &api,
            "PATCH",
            "/api/v1/snippets/red",
            Some(json!({ "enabled": false, "version": 1 })),
        )
        .await;
        assert_eq!(
            (status, &problem["code"]),
            (409, &json!("version-conflict"))
        );

        let (status, problem) = call(&api, "DELETE", "/api/v1/snippets/red?version=1", None).await;
        assert_eq!(
            (status, &problem["code"]),
            (409, &json!("version-conflict"))
        );
        let (status, _) = call(&api, "DELETE", "/api/v1/snippets/red?version=2", None).await;
        assert_eq!(status, 204);
        let (status, problem) = call(&api, "DELETE", "/api/v1/snippets/red?version=2", None).await;
        assert_eq!((status, &problem["code"]), (404, &json!("snippet-missing")));
    }

    #[tokio::test]
    async fn the_stylesheet_holds_only_enabled_snippets_as_same_origin_css() {
        let api = test_api().await;
        let (status, kind, _, text) = stylesheet(&api).await;
        assert_eq!(
            (status, kind.as_deref(), text.as_str()),
            (200, Some("text/css; charset=utf-8"), "")
        );

        call(&api, "POST", "/api/v1/snippets", create("one", "a{}")).await;
        call(
            &api,
            "POST",
            "/api/v1/snippets",
            Some(json!({ "id": "off", "name": "Off", "css": "c{}", "enabled": false })),
        )
        .await;
        call(&api, "POST", "/api/v1/snippets", create("two", "b{}")).await;
        let (status, kind, csp, text) = stylesheet(&api).await;
        assert_eq!(status, 200);
        assert_eq!(kind.as_deref(), Some("text/css; charset=utf-8"));
        assert_eq!(text, "/* snippet: one */\na{}\n/* snippet: two */\nb{}\n");
        // the page policy that lets a same-origin link load it
        let csp = csp.unwrap();
        assert!(csp.contains("style-src 'self'"), "{csp}");

        call(
            &api,
            "PATCH",
            "/api/v1/snippets/off",
            Some(json!({ "enabled": true, "version": 1 })),
        )
        .await;
        call(
            &api,
            "PATCH",
            "/api/v1/snippets/one",
            Some(json!({ "enabled": false, "version": 1 })),
        )
        .await;
        let (_, _, _, text) = stylesheet(&api).await;
        assert_eq!(text, "/* snippet: off */\nc{}\n/* snippet: two */\nb{}\n");
    }

    #[tokio::test]
    async fn bad_input_is_refused_with_stable_codes() {
        let api = test_api().await;
        let big = "a".repeat(64 * 1024 + 1);
        let cases = [
            (create("A", "a{}"), "snippet-id-invalid"),
            (create("a", "a{}"), "snippet-id-invalid"),
            (
                Some(json!({ "id": "ok", "name": " ", "css": "a{}" })),
                "snippet-name-invalid",
            ),
            (create("ok", "@import 'x.css';"), "snippet-css-forbidden"),
            (
                create("ok", "a { background: url(https://evil.test/a.png) }"),
                "snippet-css-forbidden",
            ),
            (
                create("ok", "a { background: url(javascript:alert(1)) }"),
                "snippet-css-forbidden",
            ),
            (create("ok", &big), "snippet-too-large"),
            (
                Some(json!({ "id": "ok", "name": "n", "css": "a{}", "extra": 1 })),
                "input-invalid",
            ),
            (Some(json!({ "id": "ok", "name": "n" })), "input-invalid"),
        ];
        for (body, code) in cases {
            let (status, problem) = call(&api, "POST", "/api/v1/snippets", body.clone()).await;
            assert_eq!((status, &problem["code"]), (422, &json!(code)), "{body:?}");
        }
        let (_, list) = call(&api, "GET", "/api/v1/snippets", None).await;
        assert_eq!(list, json!({ "items": [] }));

        call(&api, "POST", "/api/v1/snippets", create("ok", "a{}")).await;
        let (status, problem) = call(
            &api,
            "PATCH",
            "/api/v1/snippets/ok",
            Some(json!({ "css": "@import 'x';", "version": 1 })),
        )
        .await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("snippet-css-forbidden"))
        );
        let (status, _) = call(
            &api,
            "PATCH",
            "/api/v1/snippets/ok",
            Some(json!({ "version": 0 })),
        )
        .await;
        assert_eq!(status, 422);
        let (status, problem) = call(&api, "DELETE", "/api/v1/snippets/Bad?version=1", None).await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("snippet-id-invalid"))
        );
    }

    #[tokio::test]
    async fn an_id_is_taken_once_and_creation_says_where_it_is() {
        let api = test_api().await;
        let request = bearer(Request::post("/api/v1/snippets"), &api.token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(create("red", "a{}").unwrap().to_string()))
            .unwrap();
        let response = tower::ServiceExt::oneshot(api.router.clone(), request)
            .await
            .unwrap();
        let location = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(location, "/api/v1/snippets/red");
        // the address it names answers
        let (status, found) = call(&api, "GET", &location, None).await;
        assert_eq!((status, &found["id"]), (200, &json!("red")));
        let (status, problem) = call(&api, "GET", "/api/v1/snippets/none", None).await;
        assert_eq!((status, &problem["code"]), (404, &json!("snippet-missing")));
        let (status, problem) = call(&api, "GET", "/api/v1/snippets/Bad", None).await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("snippet-id-invalid"))
        );
        let (status, problem) = call(&api, "POST", "/api/v1/snippets", create("red", "b{}")).await;
        assert_eq!((status, &problem["code"]), (409, &json!("snippet-exists")));
    }

    #[tokio::test]
    async fn snippets_need_a_login() {
        let api = test_api().await;
        for request in [
            local(Request::get("/api/v1/snippets")),
            local(Request::get("/api/v1/snippets/active.css")),
            local(Request::post("/api/v1/snippets")),
            local(Request::delete("/api/v1/snippets/red?version=1")),
        ] {
            let (status, _, _) = send(&api.router, request.body(Body::empty()).unwrap()).await;
            assert_eq!(status, 401);
        }
    }
}
