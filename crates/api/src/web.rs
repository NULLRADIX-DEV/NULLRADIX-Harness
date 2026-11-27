use std::path::Path;

use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

use crate::{API_PREFIX, APP_CSP};

const INDEX_FILE: &str = "index.html";

/// The inline scripts of a page as CSP hash sources, in document order.
fn inline_script_hashes(html: &str) -> Vec<String> {
    let mut hashes = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<script") {
        rest = &rest[start + "<script".len()..];
        let Some(tag_end) = rest.find('>') else {
            break;
        };
        let attributes = &rest[..tag_end];
        rest = &rest[tag_end + 1..];
        let Some(close) = rest.find("</script") else {
            break;
        };
        let content = &rest[..close];
        rest = &rest[close..];
        if attributes
            .split_whitespace()
            .any(|attribute| attribute.starts_with("src="))
        {
            continue;
        }
        let digest = Sha256::digest(content.as_bytes());
        hashes.push(format!("'sha256-{}'", STANDARD.encode(digest)));
    }
    hashes
}

/// The policy for the page: the base one, plus the hash of every inline script such as the
/// import map that lets themes share the app's modules.
fn page_policy(html: &str) -> Option<HeaderValue> {
    let hashes = inline_script_hashes(html);
    if hashes.is_empty() {
        return None;
    }
    let policy = APP_CSP.replacen(
        "script-src 'self'",
        &format!("script-src 'self' {}", hashes.join(" ")),
        1,
    );
    HeaderValue::from_str(&policy).ok()
}

/// Serves the built web UI; unknown paths get `index.html` so client routes survive reloads.
pub(crate) async fn serve(dir: &Path, request: Request) -> Response {
    let hashed = request.uri().path().starts_with("/assets/");
    let service = ServeDir::new(dir).fallback(ServeFile::new(dir.join(INDEX_FILE)));
    let mut response = match service.oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(never) => match never {},
    };
    let not_modified = response.status() == StatusCode::NOT_MODIFIED;
    let shell = response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"text/html"));
    // a 304 replaces the stored headers, so a revalidated page repeats its policy
    if shell || (not_modified && !hashed) {
        let html = tokio::fs::read_to_string(dir.join(INDEX_FILE)).await;
        if let Some(policy) = html.ok().as_deref().and_then(page_policy) {
            response
                .headers_mut()
                .insert(header::CONTENT_SECURITY_POLICY, policy);
        }
    }
    // a 304 keeps the cache policy the browser already stored with its copy
    if not_modified {
        return response;
    }
    // hashed bundles never change; the shell, boot files and unknown paths are revalidated
    let policy = if hashed && response.status().is_success() && !shell {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    response
}

/// API answers carry user data and must never be cached.
pub(crate) async fn no_store_for_api(request: Request, next: Next) -> Response {
    let api = request.uri().path().starts_with(API_PREFIX);
    let mut response = next.run(request).await;
    if api {
        // a handler may choose its own policy, as theme files do
        response
            .headers_mut()
            .entry(header::CACHE_CONTROL)
            .or_insert(HeaderValue::from_static("no-store"));
    }
    response
}

pub(crate) fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::tests::{local, test_api_with_web};

    async fn get(router: &axum::Router, path: &str) -> (StatusCode, axum::http::HeaderMap, String) {
        let request = local(Request::get(path)).body(Body::empty()).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn serves_the_ui_with_a_spa_fallback_and_security_headers() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("index.html"), "<html>shell</html>").unwrap();
        fs::create_dir(dir.path().join("assets")).unwrap();
        fs::write(dir.path().join("assets").join("app.js"), "console.log(1)").unwrap();
        let api = test_api_with_web(Some(dir.path().to_path_buf())).await;

        let (status, headers, body) = get(&api.router, "/").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "<html>shell</html>");
        assert_eq!(headers["cache-control"], "no-cache");
        assert_eq!(headers["x-frame-options"], "DENY");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["referrer-policy"], "no-referrer");
        let csp = headers["content-security-policy"].to_str().unwrap();
        for part in [
            "default-src 'self'",
            "script-src 'self'",
            "worker-src 'self'",
            "frame-src 'none'",
            "form-action 'self'",
            "img-src 'self' data:",
            "font-src 'self' data:",
            "style-src 'self'",
            "connect-src 'self'",
            "media-src 'self'",
        ] {
            assert!(csp.contains(part), "{part} missing in {csp}");
        }
        assert!(!csp.contains("unsafe-eval"), "{csp}");
        let script = csp
            .split("; ")
            .find(|d| d.starts_with("script-src"))
            .unwrap();
        assert_eq!(script, "script-src 'self'");
        assert!(!csp.contains("unsafe-inline"), "{csp}");

        let (status, _, body) = get(&api.router, "/board/ARC-0001").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "<html>shell</html>");

        let (status, headers, body) = get(&api.router, "/assets/app.js").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "console.log(1)");
        assert_eq!(
            headers["cache-control"],
            "public, max-age=31536000, immutable"
        );

        fs::write(dir.path().join("boot-theme.js"), "var a;").unwrap();
        let (_, headers, _) = get(&api.router, "/boot-theme.js").await;
        assert_eq!(headers["cache-control"], "no-cache");
        let (_, headers, _) = get(&api.router, "/assets/missing.js").await;
        assert_eq!(headers["cache-control"], "no-cache");
    }

    #[tokio::test]
    async fn a_revalidated_asset_keeps_its_cached_policy() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("index.html"), "<html>shell</html>").unwrap();
        fs::create_dir(dir.path().join("assets")).unwrap();
        fs::write(dir.path().join("assets").join("app.js"), "console.log(1)").unwrap();
        let api = test_api_with_web(Some(dir.path().to_path_buf())).await;
        let (_, headers, _) = get(&api.router, "/assets/app.js").await;
        let modified = headers["last-modified"].clone();

        let request = local(Request::get("/assets/app.js"))
            .header("if-modified-since", modified)
            .body(Body::empty())
            .unwrap();
        let response = api.router.clone().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert!(response.headers().get("cache-control").is_none());
    }

    const IMPORT_MAP: &str = r#"{"imports":{"react":"/assets/host-react.js"}}"#;

    fn page(map: &str) -> String {
        format!(
            "<html><head><script src=\"/boot-theme.js\"></script>\
             <script type=\"importmap\">{map}</script>\
             <script type=\"module\" src=\"/assets/app.js\"></script></head></html>"
        )
    }

    fn hash(text: &str) -> String {
        use base64::Engine as _;
        use sha2::Digest as _;
        let digest = sha2::Sha256::digest(text.as_bytes());
        format!(
            "'sha256-{}'",
            base64::engine::general_purpose::STANDARD.encode(digest)
        )
    }

    fn script_src(headers: &axum::http::HeaderMap) -> String {
        let csp = headers["content-security-policy"].to_str().unwrap();
        assert!(!csp.contains("unsafe-eval"), "{csp}");
        assert!(!csp.contains("unsafe-inline"), "{csp}");
        assert!(!csp.contains("http"), "{csp}");
        csp.split("; ")
            .find(|directive| directive.starts_with("script-src"))
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn the_import_map_is_allowed_by_its_hash_and_nothing_else_is() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("index.html"), page(IMPORT_MAP)).unwrap();
        fs::create_dir(dir.path().join("assets")).unwrap();
        fs::write(dir.path().join("assets").join("app.js"), "console.log(1)").unwrap();
        let api = test_api_with_web(Some(dir.path().to_path_buf())).await;
        let expected = format!("script-src 'self' {}", hash(IMPORT_MAP));

        // the shell, a client route and the explicit file all carry the hash
        for path in ["/", "/board/ARC-0001", "/index.html"] {
            let (status, headers, _) = get(&api.router, path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert_eq!(script_src(&headers), expected, "{path}");
        }
        // scripts and other files need no hash
        let (_, headers, _) = get(&api.router, "/assets/app.js").await;
        assert_eq!(script_src(&headers), "script-src 'self'");

        // a revalidated page repeats the policy, or the 304 would drop the hash
        let (_, headers, _) = get(&api.router, "/").await;
        let request = local(Request::get("/"))
            .header("if-modified-since", headers["last-modified"].clone())
            .body(Body::empty())
            .unwrap();
        let response = api.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(script_src(response.headers()), expected);

        // a rebuilt page brings its own map and hash
        let other = r#"{"imports":{"react":"/assets/host-react-2.js"}}"#;
        fs::write(dir.path().join("index.html"), page(other)).unwrap();
        let (_, headers, _) = get(&api.router, "/").await;
        assert_eq!(
            script_src(&headers),
            format!("script-src 'self' {}", hash(other))
        );
    }

    #[test]
    fn only_inline_scripts_are_hashed() {
        assert!(super::inline_script_hashes("<html></html>").is_empty());
        let external =
            "<script src=\"/a.js\"></script><script type=\"module\" src=\"/b.js\"></script>";
        assert!(super::inline_script_hashes(external).is_empty());
        let two = "<script>a()</script><script src=\"/a.js\"></script><script>b()</script>";
        assert_eq!(super::inline_script_hashes(two), [hash("a()"), hash("b()")]);
        // a script that never ends is no script
        assert!(super::inline_script_hashes("<script>a()").is_empty());
        assert!(super::inline_script_hashes("<script").is_empty());
        assert_eq!(super::page_policy("<p>no scripts</p>"), None);
    }

    #[tokio::test]
    async fn api_paths_never_fall_back_to_the_ui() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("index.html"), "<html>shell</html>").unwrap();
        let api = test_api_with_web(Some(dir.path().to_path_buf())).await;
        let (status, headers, _) = get(&api.router, "/api/v2/anything").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(headers["content-type"], "application/problem+json");
    }

    #[tokio::test]
    async fn api_answers_are_not_cached() {
        let api = test_api_with_web(None).await;
        let (_, headers, _) = get(&api.router, "/api/v1/health").await;
        assert_eq!(headers["cache-control"], "no-store");
    }

    #[test]
    fn recognises_api_paths() {
        assert!(super::is_api_path("/api"));
        assert!(super::is_api_path("/api/v1/health"));
        assert!(!super::is_api_path("/apis"));
        assert!(!super::is_api_path("/board"));
    }
}
