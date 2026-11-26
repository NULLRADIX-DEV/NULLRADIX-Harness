use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;

use crate::problem;

/// Largest declared body that is read before the request is routed.
const BUFFER_LIMIT: usize = 64 * 1024;

/// Reads small bodies first so an ignored body never makes hyper reset the connection.
pub(crate) async fn buffer_small_body(request: Request, next: Next) -> Response {
    if !should_buffer(&request) {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    match to_bytes(body, BUFFER_LIMIT).await {
        Ok(bytes) => {
            next.run(Request::from_parts(parts, Body::from(bytes)))
                .await
        }
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
    request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| (1..=BUFFER_LIMIT).contains(&length))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::Router;
    use axum::http::Request;
    use axum::routing::post;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    use super::*;
    use crate::tests::{bearer, local, send, test_api};

    const BODY: &str =
        r#"{"padding":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}"#;

    async fn start(router: Router) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new().cancelled_owned();
        tokio::spawn(crate::serve(listener, router, stop));
        address
    }

    /// Reads one response off the wire and returns its status line; None when the server closed.
    async fn read_response(stream: &mut TcpStream) -> Option<String> {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0_u8; 1];
            if stream.read(&mut byte).await.ok()? == 0 {
                return None;
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8(head).unwrap();
        let length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0_u8; length];
        stream.read_exact(&mut body).await.ok()?;
        Some(head.lines().next().unwrap().to_owned())
    }

    /// Sends a POST whose body trails its head, then the same POST again, on one connection.
    async fn twice_on_one_connection(
        router: Router,
        path: &str,
        token: Option<&str>,
    ) -> [Option<String>; 2] {
        let address = start(router).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
        let head = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:4711\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            BODY.len()
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        // Without the guard the answer comes before the body is sent.
        let early = tokio::time::timeout(Duration::from_millis(300), read_response(&mut stream));
        let first = if let Ok(answer) = early.await {
            let _ = stream.write_all(BODY.as_bytes()).await;
            answer
        } else {
            stream.write_all(BODY.as_bytes()).await.unwrap();
            read_response(&mut stream).await
        };
        let again = format!("{head}{BODY}");
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
        let answers = twice_on_one_connection(api.router, path, Some(&token)).await;
        assert!(answers.iter().all(Option::is_some), "{answers:?}");
    }

    #[tokio::test]
    async fn a_request_refused_by_auth_keeps_the_connection() {
        let api = test_api().await;
        let answers = twice_on_one_connection(api.router, "/api/v1/dispatcher/resume", None).await;
        assert_eq!(answers[0].as_deref(), Some("HTTP/1.1 401 Unauthorized"));
        assert!(answers[1].is_some(), "{answers:?}");
    }

    #[tokio::test]
    async fn an_unknown_route_keeps_the_connection() {
        let api = test_api().await;
        let token = api.token.expose().to_owned();
        let answers = twice_on_one_connection(api.router, "/api/v1/nope", Some(&token)).await;
        assert_eq!(answers[0].as_deref(), Some("HTTP/1.1 404 Not Found"));
        assert!(answers[1].is_some(), "{answers:?}");
    }

    #[tokio::test]
    async fn a_body_that_fails_to_read_is_a_bad_request() {
        let api = test_api().await;
        let broken = Body::from_stream(futures_util::stream::once(async {
            Err::<Vec<u8>, std::io::Error>(std::io::Error::other("cut"))
        }));
        let request = bearer(Request::post("/api/v1/dispatcher/resume"), &api.token)
            .header(header::CONTENT_LENGTH, "10")
            .body(broken)
            .unwrap();
        let (status, content_type, json) = send(&api.router, request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(content_type.unwrap(), "application/problem+json");
        assert_eq!(json["code"], "request-body-unreadable");
    }

    #[tokio::test]
    async fn bodies_above_the_limit_reach_the_handler_unbuffered() {
        let size =
            |body: Body| async move { to_bytes(body, usize::MAX).await.unwrap().len().to_string() };
        let router = Router::new()
            .route("/", post(size))
            .layer(axum::middleware::from_fn(buffer_small_body));
        let big = vec![b'a'; BUFFER_LIMIT + 1];
        let request = local(Request::post("/"))
            .header(header::CONTENT_LENGTH, big.len())
            .body(Body::from(big))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        let body = to_bytes(response.into_body(), 64).await.unwrap();
        assert_eq!(body.as_ref(), b"65537");
    }
}
