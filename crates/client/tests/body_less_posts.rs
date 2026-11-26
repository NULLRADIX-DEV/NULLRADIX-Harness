#![allow(clippy::unwrap_used, reason = "tests fail loudly")]

use std::net::SocketAddr;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::post;
use nullradix_app::auth::Secret;
use nullradix_client::{Client, ClientError};
use uuid::Uuid;

/// Answers 409 for a request without a body and 400 for one that carries any.
async fn answer(headers: HeaderMap) -> (StatusCode, Json<serde_json::Value>) {
    let sized = headers
        .get(header::CONTENT_LENGTH)
        .is_some_and(|length| length != "0");
    let chunked = headers.contains_key(header::TRANSFER_ENCODING);
    let status = if sized || chunked {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::CONFLICT
    };
    let problem = serde_json::json!({
        "type": "about:blank", "title": "t", "status": status.as_u16(), "detail": "d", "code": "c"
    });
    (status, Json(problem))
}

async fn start() -> SocketAddr {
    let app = Router::new()
        .route("/api/v1/dispatcher/resume", post(answer))
        .route("/api/v1/dispatcher/passes", post(answer))
        .route("/api/v1/accounts/{id}/resume", post(answer))
        .route("/api/v1/questions/{key}/research-runs", post(answer))
        .route("/api/v1/tickets/{key}/plan-runs", post(answer));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

fn client(address: SocketAddr) -> Client {
    let timeout = Duration::from_secs(10);
    Client::new(address, timeout, timeout).with_token(Secret::parse(&"a".repeat(64)).unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn posts_without_input_send_no_body() {
    let address = start().await;
    let statuses = tokio::task::spawn_blocking(move || {
        let client = client(address);
        [
            client.resume_dispatcher().map(drop),
            client.run_dispatcher_pass().map(drop),
            client.resume_account(Uuid::nil()).map(drop),
            client.start_research_run("ARC-Q1").map(drop),
            client.start_plan_run("ARC-1").map(drop),
        ]
        .map(|result| match result {
            Err(ClientError::Refused { status, .. }) => status,
            other => panic!("unexpected {other:?}"),
        })
    })
    .await
    .unwrap();
    assert_eq!(statuses, [409; 5]);
}
