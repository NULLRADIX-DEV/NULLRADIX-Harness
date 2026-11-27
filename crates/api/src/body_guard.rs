use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use futures_util::{StreamExt, stream};

use crate::problem;

/// Largest body that is read before the request is routed.
const BUFFER_LIMIT: usize = 64 * 1024;

/// Reads small bodies first so an ignored body never makes hyper reset the connection.
pub(crate) async fn buffer_small_body(request: Request, next: Next) -> Response {
    if !should_buffer(&request) {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    match read_prefix(body).await {
        Ok(body) => next.run(Request::from_parts(parts, body)).await,
        Err(_) => problem::response(
            StatusCode::BAD_REQUEST,
            "request-body-unreadable",
            "the request body could not be read",
        ),
    }
}

fn should_buffer(request: &Request) -> bool {
    if matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE | Method::CONNECT
    ) {
        return false;
    }
    let headers = request.headers();
    // chunked bodies have no length; read up to 64 KiB
    if headers.contains_key(header::TRANSFER_ENCODING) {
        return true;
    }
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| (1..=BUFFER_LIMIT).contains(&length))
}

/// The whole body within the limit, else the read prefix chained with the rest.
async fn read_prefix(body: Body) -> Result<Body, axum::Error> {
    let mut rest = body.into_data_stream();
    let mut prefix = Vec::new();
    while let Some(chunk) = rest.next().await {
        let mut chunk = chunk?;
        let room = BUFFER_LIMIT - prefix.len();
        if chunk.len() > room {
            prefix.extend_from_slice(&chunk.split_to(room));
            let read = stream::iter([Ok::<_, axum::Error>(Bytes::from(prefix)), Ok(chunk)]);
            return Ok(Body::from_stream(read.chain(rest)));
        }
        prefix.extend_from_slice(&chunk);
    }
    Ok(Body::from(prefix))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use axum::Router;
    use axum::body::to_bytes;
    use axum::http::Request;
    use axum::routing::post;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;
    use tower::ServiceExt;

    use super::*;
    use crate::Limits;
    use crate::server::wire::{read_response, start};
    use crate::tests::{bearer, local, send, test_api, test_api_with};

    const BODY: &str =
        r#"{"padding":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}"#;

    #[derive(Clone, Copy)]
    enum Framing {
        Length,
        Chunked,
    }

    /// Sends a POST whose body trails its head, then the same POST again, on one connection.
    async fn twice_on_one_connection(
        router: Router,
        path: &str,
        token: Option<&str>,
        framing: Framing,
    ) -> [Option<String>; 2] {
        let address = start(router, Limits::default()).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
        let (framing_header, body) = match framing {
            Framing::Length => (format!("Content-Length: {}", BODY.len()), BODY.to_owned()),
            Framing::Chunked => (
                String::from("Transfer-Encoding: chunked"),
                format!("{:x}\r\n{BODY}\r\n0\r\n\r\n", BODY.len()),
            ),
        };
        let head = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:4711\r\n{auth}Content-Type: application/json\r\n{framing_header}\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        // Without the guard the answer comes before the body is sent.
        let early = tokio::time::timeout(Duration::from_millis(300), read_response(&mut stream));
        let first = if let Ok(answer) = early.await {
            let _ = stream.write_all(body.as_bytes()).await;
            answer
        } else {
            stream.write_all(body.as_bytes()).await.unwrap();
            read_response(&mut stream).await
        };
        let again = format!("{head}{body}");
        if first.is_none() || stream.write_all(again.as_bytes()).await.is_err() {
            return [first, None];
        }
        let second = tokio::time::timeout(Duration::from_secs(5), read_response(&mut stream))
            .await
            .unwrap();
        [first, second]
    }

    #[tokio::test]
    async fn a_handler_that_ignores_its_body_keeps_the_connection() {
        let api = test_api().await;
        let token = api.token.expose().to_owned();
        let path = "/api/v1/dispatcher/resume";
        let answers =
            twice_on_one_connection(api.router, path, Some(&token), Framing::Length).await;
        assert!(answers.iter().all(Option::is_some), "{answers:?}");
    }

    #[tokio::test]
    async fn a_handler_that_ignores_a_chunked_body_keeps_the_connection() {
        let api = test_api().await;
        let token = api.token.expose().to_owned();
        let path = "/api/v1/dispatcher/resume";
        let answers =
            twice_on_one_connection(api.router, path, Some(&token), Framing::Chunked).await;
        assert_eq!(answers[0].as_deref(), Some("HTTP/1.1 200 OK"));
        assert!(answers[1].is_some(), "{answers:?}");
    }

    #[tokio::test]
    async fn a_request_refused_by_auth_keeps_the_connection() {
        let api = test_api().await;
        let path = "/api/v1/dispatcher/resume";
        let answers = twice_on_one_connection(api.router, path, None, Framing::Length).await;
        assert_eq!(answers[0].as_deref(), Some("HTTP/1.1 401 Unauthorized"));
        assert!(answers[1].is_some(), "{answers:?}");
    }

    #[tokio::test]
    async fn a_chunked_request_refused_by_auth_keeps_the_connection() {
        let api = test_api().await;
        let path = "/api/v1/dispatcher/resume";
        let answers = twice_on_one_connection(api.router, path, None, Framing::Chunked).await;
        assert_eq!(answers[0].as_deref(), Some("HTTP/1.1 401 Unauthorized"));
        assert!(answers[1].is_some(), "{answers:?}");
    }

    #[tokio::test]
    async fn an_unknown_route_keeps_the_connection() {
        let api = test_api().await;
        let token = api.token.expose().to_owned();
        let path = "/api/v1/nope";
        let answers =
            twice_on_one_connection(api.router, path, Some(&token), Framing::Length).await;
        assert_eq!(answers[0].as_deref(), Some("HTTP/1.1 404 Not Found"));
        assert!(answers[1].is_some(), "{answers:?}");
    }

    fn broken_body() -> Body {
        Body::from_stream(futures_util::stream::once(async {
            Err::<Vec<u8>, std::io::Error>(std::io::Error::other("cut"))
        }))
    }

    #[tokio::test]
    async fn a_body_that_fails_to_read_is_a_bad_request_with_the_edge_headers() {
        let api = test_api().await;
        let request = bearer(Request::post("/api/v1/dispatcher/resume"), &api.token)
            .header(header::CONTENT_LENGTH, "10")
            .body(broken_body())
            .unwrap();
        let response = api.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let headers = response.headers();
        assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
        assert!(headers.contains_key("x-request-id"));
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "request-body-unreadable");
    }

    #[tokio::test]
    async fn a_chunked_body_that_fails_to_read_is_a_bad_request() {
        let api = test_api().await;
        let request = bearer(Request::post("/api/v1/dispatcher/resume"), &api.token)
            .header(header::TRANSFER_ENCODING, "chunked")
            .body(broken_body())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["code"], "request-body-unreadable");
    }

    #[tokio::test]
    async fn a_foreign_host_is_refused_before_its_body_is_read() {
        let api = test_api().await;
        let polled = Arc::new(AtomicBool::new(false));
        let seen = polled.clone();
        let body = Body::from_stream(futures_util::stream::poll_fn(move |_| {
            seen.store(true, Ordering::SeqCst);
            std::task::Poll::Ready(None::<Result<Vec<u8>, std::io::Error>>)
        }));
        let request = Request::post("/api/v1/dispatcher/resume")
            .header(header::HOST, "attacker.example.com:4711")
            .header(header::CONTENT_LENGTH, "10")
            .body(body)
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["code"], "host-not-allowed");
        assert!(!polled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_body_that_never_arrives_ends_with_a_timeout() {
        let limits = Limits::new(Duration::from_secs(10), Duration::from_millis(200), 8, 8);
        let api = test_api_with(None, limits).await;
        let stalled = Body::from_stream(futures_util::stream::pending::<
            Result<Vec<u8>, std::io::Error>,
        >());
        let request = bearer(Request::post("/api/v1/dispatcher/resume"), &api.token)
            .header(header::CONTENT_LENGTH, "10")
            .body(stalled)
            .unwrap();
        let answer =
            tokio::time::timeout(Duration::from_secs(30), api.router.clone().oneshot(request));
        let response = answer.await.expect("the request was ended").unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().contains_key("x-request-id"));
    }

    /// What a handler sees of a body: its length and a fingerprint of its bytes.
    async fn fingerprint(body: Body) -> String {
        let bytes = to_bytes(body, usize::MAX).await.unwrap();
        let sum: u64 = bytes
            .iter()
            .zip(1_u64..)
            .map(|(byte, position)| u64::from(*byte) * position)
            .sum();
        format!("{}:{sum}", bytes.len())
    }

    async fn through_the_guard(chunks: &[Vec<u8>], framing: (header::HeaderName, &str)) -> String {
        let router = Router::new()
            .route("/", post(fingerprint))
            .layer(axum::middleware::from_fn(buffer_small_body));
        let chunks: Vec<Result<Vec<u8>, std::io::Error>> = chunks.iter().cloned().map(Ok).collect();
        let request = local(Request::post("/"))
            .header(framing.0, framing.1)
            .body(Body::from_stream(futures_util::stream::iter(chunks)))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        let body = to_bytes(response.into_body(), 64).await.unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    fn pattern(length: usize) -> Vec<u8> {
        (0..length)
            .map(|at| u8::try_from(at % 251).unwrap())
            .collect()
    }

    const CHUNKED: (header::HeaderName, &str) = (header::TRANSFER_ENCODING, "chunked");

    #[tokio::test]
    async fn a_chunked_body_within_the_limit_reaches_the_handler_whole() {
        let bytes = pattern(BUFFER_LIMIT);
        let chunks: Vec<Vec<u8>> = bytes.chunks(5000).map(<[u8]>::to_vec).collect();
        let seen = through_the_guard(&chunks, CHUNKED).await;
        assert_eq!(seen, fingerprint(Body::from(bytes)).await);
    }

    #[tokio::test]
    async fn a_chunked_body_above_the_limit_reaches_the_handler_whole() {
        let bytes = pattern(BUFFER_LIMIT + 1);
        let chunks: Vec<Vec<u8>> = bytes.chunks(40_000).map(<[u8]>::to_vec).collect();
        let seen = through_the_guard(&chunks, CHUNKED).await;
        assert_eq!(seen, fingerprint(Body::from(bytes)).await);
    }

    #[tokio::test]
    async fn a_chunk_across_the_limit_is_split_and_kept_in_order() {
        let bytes = pattern(BUFFER_LIMIT * 3 + 17);
        let chunks: Vec<Vec<u8>> = bytes.chunks(BUFFER_LIMIT - 5).map(<[u8]>::to_vec).collect();
        let seen = through_the_guard(&chunks, CHUNKED).await;
        assert_eq!(seen, fingerprint(Body::from(bytes)).await);
    }

    #[tokio::test]
    async fn bodies_above_the_limit_with_a_length_reach_the_handler_unbuffered() {
        let bytes = pattern(BUFFER_LIMIT + 1);
        let length = bytes.len().to_string();
        let seen = through_the_guard(
            std::slice::from_ref(&bytes),
            (header::CONTENT_LENGTH, &length),
        )
        .await;
        assert_eq!(seen, fingerprint(Body::from(bytes)).await);
    }
}
