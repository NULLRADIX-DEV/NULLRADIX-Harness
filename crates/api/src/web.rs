use std::path::Path;

use axum::extract::Request;
use axum::http::{HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

use crate::API_PREFIX;

/// Serves the built web UI; unknown paths get `index.html` so client routes survive reloads.
pub(crate) async fn serve(dir: &Path, request: Request) -> Response {
    let hashed = request.uri().path().starts_with("/assets/");
    let service = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
    let mut response = match service.oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(never) => match never {},
    };
    // hashed bundles never change; the shell, boot files and unknown paths are revalidated
    let shell = response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"text/html"));
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
