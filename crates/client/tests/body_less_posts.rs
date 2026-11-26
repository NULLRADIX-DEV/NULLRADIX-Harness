#![allow(clippy::unwrap_used, reason = "tests fail loudly")]

use std::net::SocketAddr;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::routing::post;
use nullradix_app::auth::Secret;
use nullradix_client::{Client, ClientError};
use uuid::Uuid;

/// A POST route that declares no body answers at once without reading one.
async fn answer() -> (StatusCode, Json<serde_json::Value>) {
    let problem = serde_json::json!({
        "type": "about:blank", "title": "t", "status": 409, "detail": "d", "code": "c"
    });
    (StatusCode::CONFLICT, Json(problem))
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn posts_without_input_are_not_reset_by_a_daemon_that_reads_no_body() {
    let address = start().await;
    let mut tasks = Vec::new();
    for _ in 0..8 {
        tasks.push(tokio::task::spawn_blocking(move || {
            let mut lost = Vec::new();
            for _ in 0..40 {
                let client = client(address);
                let results = [
                    client.resume_dispatcher().map(drop),
                    client.run_dispatcher_pass().map(drop),
                    client.resume_account(Uuid::nil()).map(drop),
                    client.start_research_run("ARC-Q1").map(drop),
                    client.start_plan_run("ARC-1").map(drop),
                ];
                for result in results {
                    match result {
                        Err(ClientError::Refused { .. }) => {}
                        other => lost.push(format!("{other:?}")),
                    }
                }
            }
            lost
        }));
    }
    let mut lost = Vec::new();
    for task in tasks {
        lost.extend(task.await.unwrap());
    }
    assert!(
        lost.is_empty(),
        "{} lost, first: {:?}",
        lost.len(),
        lost.first()
    );
}
