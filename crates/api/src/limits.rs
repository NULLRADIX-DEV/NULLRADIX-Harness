use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{API_PREFIX, problem};

/// A client gets this long for a request head, also between requests on one connection.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Above a theme download budget of 120 s; downloads queued behind four others may time out.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(150);
/// Far above a person's tabs, CLI and hooks, and below a Unix soft limit of 256 files.
const MAX_CONNECTIONS: usize = 128;
/// One stream per open tab; the rest of the connections stay free for requests.
const MAX_EVENT_STREAMS: usize = 32;

/// Time and count limits of the HTTP edge.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Time to send a request head; a slower client is dropped.
    pub header_read_timeout: Duration,
    /// Time to answer a request, body included; event streams are exempt.
    pub request_timeout: Duration,
    /// Connections served at once; more wait in the accept queue.
    pub max_connections: usize,
    streams: Arc<Semaphore>,
}

impl Limits {
    #[must_use]
    pub fn new(
        header_read_timeout: Duration,
        request_timeout: Duration,
        max_connections: usize,
        max_event_streams: usize,
    ) -> Self {
        Self {
            header_read_timeout,
            request_timeout,
            max_connections: max_connections.max(1),
            streams: Arc::new(Semaphore::new(max_event_streams)),
        }
    }

    /// A place for one more event stream, held until the stream ends.
    pub(crate) fn event_stream(&self) -> Option<OwnedSemaphorePermit> {
        self.streams.clone().try_acquire_owned().ok()
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(
            HEADER_READ_TIMEOUT,
            REQUEST_TIMEOUT,
            MAX_CONNECTIONS,
            MAX_EVENT_STREAMS,
        )
    }
}

fn is_event_stream(request: &Request) -> bool {
    request.method() == Method::GET
        && request.uri().path().strip_prefix(API_PREFIX) == Some("/events")
}

/// Ends slow requests; the event stream is exempt.
pub(crate) async fn limit_request_time(
    State(limit): State<Duration>,
    request: Request,
    next: Next,
) -> Response {
    if is_event_stream(&request) {
        return next.run(request).await;
    }
    match tokio::time::timeout(limit, next.run(request)).await {
        Ok(response) => response,
        // 408 makes Chromium resend the request on a reused connection
        Err(_) => problem::response(
            StatusCode::SERVICE_UNAVAILABLE,
            "request-timeout",
            "the request took too long",
        ),
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use tower::ServiceExt;

    use super::*;

    async fn after(delay: Duration) -> &'static str {
        tokio::time::sleep(delay).await;
        "late"
    }

    fn router(limit: Duration) -> Router {
        Router::new()
            .route("/api/v1/events", get(move || after(limit * 3)))
            .route("/api/v1/other", get(move || after(limit * 3)))
            .route("/api/v1/quick", get(|| after(Duration::ZERO)))
            .layer(from_fn_with_state(limit, limit_request_time))
    }

    async fn status(router: Router, path: &str) -> StatusCode {
        let request = Request::get(path).body(Body::empty()).unwrap();
        router.oneshot(request).await.unwrap().status()
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_request_ends_with_a_problem() {
        let limit = Duration::from_millis(100);
        let response = router(limit)
            .oneshot(Request::get("/api/v1/other").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "request-timeout");
    }

    #[tokio::test(start_paused = true)]
    async fn the_event_stream_and_quick_requests_are_not_cut() {
        let limit = Duration::from_millis(100);
        assert_eq!(
            status(router(limit), "/api/v1/events").await,
            StatusCode::OK
        );
        assert_eq!(status(router(limit), "/api/v1/quick").await, StatusCode::OK);
    }

    #[test]
    fn the_stream_cap_frees_a_place_when_a_stream_ends() {
        let limits = Limits::new(Duration::from_secs(1), Duration::from_secs(1), 4, 2);
        let first = limits.event_stream();
        let second = limits.event_stream();
        assert!(first.is_some() && second.is_some());
        assert!(limits.event_stream().is_none());
        drop(first);
        assert!(limits.event_stream().is_some());
    }
}
