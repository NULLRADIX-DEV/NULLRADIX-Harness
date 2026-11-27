//! The asset proxy: resources a theme with its code on loads from the domains it declares.

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use nullradix_app::theme_fetch::AssetRequest;
use serde::Deserialize;
use utoipa::IntoParams;

use crate::themes::ASSET_CSP;
use crate::{ApiError, ApiPath, ApiQuery, ApiState, Problem};

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct FetchPath {
    /// Theme id.
    id: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct FetchQuery {
    /// https address of the resource; its host must be a domain the theme declares.
    url: String,
    /// Expected sha256 checksum (hex) of the resource, or of the archive when `entry` is set.
    sha256: Option<String>,
    /// Path of a file inside the zip archive at `url`.
    entry: Option<String>,
}

#[utoipa::path(
    get,
    path = "/themes/{id}/fetch",
    tag = "themes",
    params(FetchPath, FetchQuery),
    responses(
        (status = 200, description = "The bytes of the resource, typed by the extension; scripts, styles and unknown types are `application/octet-stream`", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 401, body = Problem),
        (status = 403, description = "`theme-fetch-forbidden`: the code switch of the theme is off, or the host or a redirect is not a declared domain; `theme-fetch-address-refused`: the host resolves to a private address; `theme-fetch-redirect-refused`: a redirect leaves public https", body = Problem),
        (status = 404, description = "`theme-not-found`: the theme is not installed; `theme-fetch-entry-not-found`: no such file in the archive", body = Problem),
        (status = 422, description = "`theme-fetch-url-invalid`, `theme-fetch-entry-invalid`, `theme-fetch-checksum-invalid`; `theme-fetch-checksum-mismatch`; `theme-fetch-too-large` above 10 MiB, or 64 MiB for an archive", body = Problem),
        (status = 503, description = "The host failed: `theme-fetch-unreachable`, `theme-fetch-timeout`, `theme-fetch-upstream-status`, `theme-fetch-redirects` (more than 5), `theme-fetch-not-archive`", body = Problem),
    )
)]
pub(crate) async fn fetch_theme_asset(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<FetchPath>,
    ApiQuery(query): ApiQuery<FetchQuery>,
) -> Result<Response, ApiError> {
    let asset = state
        .theme_assets
        .fetch(AssetRequest {
            theme: &path.id,
            url: &query.url,
            sha256: query.sha256.as_deref(),
            entry: query.entry.as_deref(),
        })
        .await?;
    Ok((
        [
            (header::CONTENT_TYPE, asset.content_type),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            // the files of another origin must not run script or load anything on the app origin
            (header::CONTENT_SECURITY_POLICY, ASSET_CSP),
            // the proxy keeps its own copy; the browser may keep one for a day
            (header::CACHE_CONTROL, "private, max-age=86400"),
        ],
        asset.data,
    )
        .into_response())
}

#[cfg(test)]
pub(crate) mod fake {
    use std::collections::HashMap;

    use async_trait::async_trait;
    use nullradix_app::theme_catalog::FetchError;
    use nullradix_app::theme_fetch::{AssetFetcher, EntryError, Hop, ZipEntries};
    use nullradix_domain::CatalogUrl;
    use parking_lot::Mutex;

    /// Answers by address with its hops in turn; a missing answer is unreachable.
    #[derive(Default)]
    pub(crate) struct Hosts {
        answers: Mutex<HashMap<String, Vec<Result<Hop, FetchError>>>>,
        pub(crate) asked: Mutex<Vec<String>>,
    }

    impl Hosts {
        pub(crate) fn serve(&self, url: &str, hops: Vec<Result<Hop, FetchError>>) {
            self.answers.lock().insert(url.to_owned(), hops);
        }

        pub(crate) fn body(&self, url: &str, bytes: &[u8]) {
            self.serve(url, vec![Ok(Hop::Body(bytes.to_vec()))]);
        }
    }

    #[async_trait]
    impl AssetFetcher for Hosts {
        async fn get(&self, url: &CatalogUrl, _max_bytes: u64) -> Result<Hop, FetchError> {
            self.asked.lock().push(url.to_string());
            let mut answers = self.answers.lock();
            match answers.get_mut(url.as_str()) {
                Some(hops) if !hops.is_empty() => hops.remove(0),
                _ => Err(FetchError::Unreachable),
            }
        }
    }

    /// Archives are `name=text` lines.
    pub(crate) struct Lines;

    #[async_trait]
    impl ZipEntries for Lines {
        async fn entry(
            &self,
            archive: Vec<u8>,
            path: &str,
            max_bytes: u64,
        ) -> Result<Vec<u8>, EntryError> {
            let text = String::from_utf8(archive).map_err(|_| EntryError::NotArchive)?;
            if !text.contains('=') {
                return Err(EntryError::NotArchive);
            }
            let found = text
                .lines()
                .filter_map(|line| line.split_once('='))
                .find(|(name, _)| *name == path)
                .map(|(_, data)| data.as_bytes().to_vec())
                .ok_or(EntryError::NotFound)?;
            if found.len() as u64 > max_bytes {
                return Err(EntryError::TooLarge);
            }
            Ok(found)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use nullradix_app::theme_catalog::FetchError;
    use nullradix_app::theme_fetch::Hop;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    use crate::tests::{TestApi, bearer, local, open_session, send, test_api};

    const MANIFEST: &str = r##"{
        "id": "night-blue", "name": "Night Blue", "version": "1.2.0", "author": "Ada",
        "sdkVersion": "^1.3", "colorScheme": "dark", "startColor": "#101018",
        "slots": [], "permissions": { "network": ["cdn.example.com", "*.assets.test"] }
    }"##;
    const ICON: &str = "https://cdn.example.com/icon.png";
    const FETCH: &str = "/api/v1/themes/night-blue/fetch";

    fn package(with_code: bool) -> Vec<u8> {
        let mut text = format!("manifest.json={MANIFEST}\n--\ntheme.css=:root {{ --a: 1 }}");
        if with_code {
            text.push_str("\n--\nmain.js=run()");
        }
        text.into_bytes()
    }

    async fn install(api: &TestApi, with_code: bool) {
        let cookie = open_session(api).await;
        let request = local(Request::post("/api/v1/themes?confirmed=true"))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, "http://127.0.0.1:4711")
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(package(with_code)))
            .unwrap();
        let (status, _, theme) = send(&api.router, request).await;
        assert_eq!(status, 201, "{theme}");
    }

    async fn code(api: &TestApi, enabled: bool) {
        let cookie = open_session(api).await;
        let request = local(Request::put("/api/v1/themes/night-blue/code"))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, "http://127.0.0.1:4711")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "enabled": enabled, "version": 1 }).to_string(),
            ))
            .unwrap();
        let (status, _, body) = send(&api.router, request).await;
        assert_eq!(status, 200, "{body}");
    }

    /// A theme with code and domains whose switch the person turned on.
    async fn ready() -> TestApi {
        let api = test_api().await;
        install(&api, true).await;
        code(&api, true).await;
        api
    }

    struct Reply {
        status: u16,
        headers: axum::http::HeaderMap,
        body: Vec<u8>,
    }

    impl Reply {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers.get(name).and_then(|value| value.to_str().ok())
        }

        fn code(&self) -> Option<String> {
            let problem: Value = serde_json::from_slice(&self.body).ok()?;
            problem["code"].as_str().map(str::to_owned)
        }
    }

    fn encoded(text: &str) -> String {
        text.bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    char::from(byte).to_string()
                }
                other => format!("%{other:02X}"),
            })
            .collect()
    }

    async fn reply_to(api: &TestApi, request: Request<Body>) -> Reply {
        let response = api.router.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        Reply {
            status,
            headers,
            body: body.to_vec(),
        }
    }

    async fn get(api: &TestApi, path: &str) -> Reply {
        let request = bearer(Request::get(path), &api.token)
            .body(Body::empty())
            .unwrap();
        reply_to(api, request).await
    }

    fn hex_sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .fold(String::new(), |mut text, byte| {
                let _ = write!(text, "{byte:02x}");
                text
            })
    }

    fn url(target: &str) -> String {
        format!("{FETCH}?url={}", encoded(target))
    }

    #[tokio::test]
    async fn a_declared_resource_is_served_with_a_restrictive_policy() {
        let api = ready().await;
        api.assets.body(ICON, b"PNG");
        let reply = get(&api, &url(ICON)).await;
        assert_eq!(
            reply.status,
            200,
            "{:?}",
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(reply.body, b"PNG");
        assert_eq!(reply.header("content-type"), Some("image/png"));
        assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(
            reply.header("cache-control"),
            Some("private, max-age=86400")
        );
        let csp = reply.header("content-security-policy").unwrap();
        assert!(csp.starts_with("sandbox; default-src 'none'"), "{csp}");
        assert!(!csp.contains("script-src"), "{csp}");
        assert!(reply.headers.get("set-cookie").is_none());
    }

    #[tokio::test]
    async fn scripts_and_styles_from_outside_are_opaque_bytes() {
        let api = ready().await;
        for name in ["run.js", "look.css", "page.html"] {
            let address = format!("https://cdn.example.com/{name}");
            api.assets.body(&address, b"x");
            let reply = get(&api, &url(&address)).await;
            assert_eq!(reply.status, 200, "{name}");
            assert_eq!(
                reply.header("content-type"),
                Some("application/octet-stream"),
                "{name}"
            );
            assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
        }
    }

    #[tokio::test]
    async fn a_file_of_an_archive_is_served_by_its_entry_path() {
        let api = ready().await;
        let jar = "https://x.assets.test/client.jar";
        api.assets.body(jar, b"a/stone.png=ROCK\n");
        let path = format!("{}&entry={}", url(jar), encoded("a/stone.png"));
        let reply = get(&api, &path).await;
        assert_eq!(
            (reply.status, reply.body.as_slice()),
            (200, b"ROCK".as_slice())
        );
        assert_eq!(reply.header("content-type"), Some("image/png"));
        let missing = format!("{}&entry=nothing.png", url(jar));
        api.assets.body(jar, b"a/stone.png=ROCK\n");
        let reply = get(&api, &missing).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (404, Some("theme-fetch-entry-not-found"))
        );
        api.assets
            .body("https://x.assets.test/plain.zip", b"no archive");
        let plain = format!("{}&entry=a.png", url("https://x.assets.test/plain.zip"));
        let reply = get(&api, &plain).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (503, Some("theme-fetch-not-archive"))
        );
    }

    #[tokio::test]
    async fn the_route_needs_a_login_and_a_known_theme() {
        let api = ready().await;
        api.assets.body(ICON, b"PNG");
        let request = local(Request::get(url(ICON))).body(Body::empty()).unwrap();
        let reply = reply_to(&api, request).await;
        assert_eq!(reply.status, 401);

        let reply = get(&api, &url(ICON).replace("night-blue", "ghost")).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (404, Some("theme-not-found"))
        );
        let reply = get(&api, &url(ICON).replace("night-blue", "Not_An_Id")).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (404, Some("theme-not-found"))
        );
        assert!(api.assets.asked.lock().is_empty());
    }

    #[tokio::test]
    async fn a_browser_session_may_load_assets_too() {
        let api = ready().await;
        api.assets.body(ICON, b"PNG");
        let cookie = open_session(&api).await;
        let request = local(Request::get(url(ICON)))
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let reply = reply_to(&api, request).await;
        assert_eq!(
            (reply.status, reply.body.as_slice()),
            (200, b"PNG".as_slice())
        );
    }

    #[tokio::test]
    async fn a_theme_without_its_code_on_is_forbidden() {
        let api = test_api().await;
        install(&api, true).await;
        api.assets.body(ICON, b"PNG");
        // installed with code, the switch is still off
        let reply = get(&api, &url(ICON)).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (403, Some("theme-fetch-forbidden"))
        );
        code(&api, true).await;
        assert_eq!(get(&api, &url(ICON)).await.status, 200);
        let off = local(Request::put("/api/v1/themes/night-blue/code"))
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", api.token.expose()),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "enabled": false }).to_string()))
            .unwrap();
        let (status, _, _) = send(&api.router, off).await;
        assert_eq!(status, 200);
        let reply = get(&api, &url(ICON)).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (403, Some("theme-fetch-forbidden"))
        );
        assert_eq!(api.assets.asked.lock().len(), 1);
    }

    #[tokio::test]
    async fn a_theme_without_code_is_forbidden() {
        let api = test_api().await;
        install(&api, false).await;
        let reply = get(&api, &url(ICON)).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (403, Some("theme-fetch-forbidden"))
        );
        assert!(api.assets.asked.lock().is_empty());
    }

    #[tokio::test]
    async fn a_host_that_is_not_declared_is_forbidden_before_any_request() {
        let api = ready().await;
        for address in [
            "https://other.example.com/a.png",
            "https://cdn.example.com.evil.test/a.png",
            "https://assets.test/a.png",
        ] {
            let reply = get(&api, &url(address)).await;
            assert_eq!(
                (reply.status, reply.code().as_deref()),
                (403, Some("theme-fetch-forbidden")),
                "{address}"
            );
        }
        assert!(api.assets.asked.lock().is_empty());
    }

    #[tokio::test]
    async fn a_redirect_to_a_host_that_is_not_declared_is_forbidden() {
        let api = ready().await;
        let moved = |to: &str| vec![Ok(Hop::Redirect(to.to_owned()))];
        api.assets.serve(ICON, moved("https://evil.test/a.png"));
        let reply = get(&api, &url(ICON)).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (403, Some("theme-fetch-forbidden"))
        );
        api.assets
            .serve(ICON, moved("http://cdn.example.com/a.png"));
        let reply = get(&api, &url(ICON)).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (403, Some("theme-fetch-redirect-refused"))
        );
        assert_eq!(api.assets.asked.lock().len(), 2);
    }

    #[tokio::test]
    async fn addresses_that_are_not_https_public_and_plain_are_refused_without_a_request() {
        let api = ready().await;
        for address in [
            "http://cdn.example.com/a.png",
            "https://cdn.example.com:8443/a.png",
            "https://user:pw@cdn.example.com/a.png",
            "https://127.0.0.1/a.png",
            "file:///etc/passwd",
            "ftp://cdn.example.com/a.png",
            "not a url",
        ] {
            let reply = get(&api, &url(address)).await;
            assert_eq!(
                (reply.status, reply.code().as_deref()),
                (422, Some("theme-fetch-url-invalid")),
                "{address}"
            );
        }
        let reply = get(&api, FETCH).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (422, Some("query-invalid"))
        );
        let reply = get(&api, &format!("{FETCH}?url=")).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (422, Some("theme-fetch-url-invalid"))
        );
        assert!(api.assets.asked.lock().is_empty());
    }

    #[tokio::test]
    async fn a_host_that_resolves_to_a_private_address_is_forbidden() {
        let api = ready().await;
        api.assets.serve(ICON, vec![Err(FetchError::Refused)]);
        let reply = get(&api, &url(ICON)).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (403, Some("theme-fetch-address-refused"))
        );
    }

    #[tokio::test]
    async fn a_wrong_checksum_and_a_bad_one_are_refused() {
        let api = ready().await;
        api.assets.body(ICON, b"PNG");
        let wrong = format!("{}&sha256={}", url(ICON), "0".repeat(64));
        let reply = get(&api, &wrong).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (422, Some("theme-fetch-checksum-mismatch"))
        );
        let bad = format!("{}&sha256=abc", url(ICON));
        let reply = get(&api, &bad).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (422, Some("theme-fetch-checksum-invalid"))
        );
        api.assets.body(ICON, b"PNG");
        let right = format!("{}&sha256={}", url(ICON), hex_sha256(b"PNG"));
        let reply = get(&api, &right).await;
        assert_eq!(
            (reply.status, reply.body.as_slice()),
            (200, b"PNG".as_slice())
        );
    }

    #[tokio::test]
    async fn entry_paths_that_are_not_plain_are_refused_without_a_request() {
        let api = ready().await;
        let jar = "https://x.assets.test/client.jar";
        for entry in [
            "..%2Fa.png",
            "%2Fa.png",
            "a%5Cb.png",
            "C%3A%2Fa.png",
            "",
            "a%2F",
        ] {
            let reply = get(&api, &format!("{}&entry={entry}", url(jar))).await;
            assert_eq!(
                (reply.status, reply.code().as_deref()),
                (422, Some("theme-fetch-entry-invalid")),
                "{entry:?}"
            );
        }
        assert!(api.assets.asked.lock().is_empty());
    }

    #[tokio::test]
    async fn failures_of_the_host_answer_503_with_their_own_codes() {
        let api = ready().await;
        for (failure, code) in [
            (FetchError::Timeout, "theme-fetch-timeout"),
            (FetchError::Unreachable, "theme-fetch-unreachable"),
            (FetchError::Status(404), "theme-fetch-upstream-status"),
        ] {
            api.assets.serve(ICON, vec![Err(failure)]);
            let reply = get(&api, &url(ICON)).await;
            assert_eq!((reply.status, reply.code().as_deref()), (503, Some(code)));
        }
        api.assets.serve(ICON, vec![Err(FetchError::TooLarge)]);
        let reply = get(&api, &url(ICON)).await;
        assert_eq!(
            (reply.status, reply.code().as_deref()),
            (422, Some("theme-fetch-too-large"))
        );
    }
}
