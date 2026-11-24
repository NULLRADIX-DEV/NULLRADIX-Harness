use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::Stream;
use futures_util::stream;
use nullradix_app::{AppError, Change, ChangeFeed};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio::time::{Interval, MissedTickBehavior};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{ApiError, ApiQuery, ApiState, Problem};

/// How far a client may lag behind before it must reload instead of replaying.
const MAX_BACKLOG: u32 = 1000;
const PAGE: u32 = 200;
/// Safety net for a missed wake-up; the bus normally wakes streams at once.
const POLL: Duration = Duration::from_secs(5);

/// A committed change, sent as the `change` event; `id` carries `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeDto {
    pub seq: i64,
    pub entity: String,
    pub entity_id: Uuid,
    pub op: String,
    pub at: i64,
}

impl From<&Change> for ChangeDto {
    fn from(change: &Change) -> Self {
        Self {
            seq: change.seq,
            entity: change.entity.as_str().to_owned(),
            entity_id: change.entity_id,
            op: change.op.as_str().to_owned(),
            at: change.at.as_millis(),
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub(crate) struct EventsQuery {
    /// Replay changes after this sequence number; `Last-Event-ID` takes precedence.
    since: Option<i64>,
}

/// Where a stream starts and whether the client must drop its cached state first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Resume {
    after: i64,
    reset: bool,
}

async fn plan(feed: &dyn ChangeFeed, since: Option<i64>, max: u32) -> Result<Resume, AppError> {
    let head = feed.head().await?;
    let Some(since) = since else {
        return Ok(Resume {
            after: head,
            reset: false,
        });
    };
    let lag = head.saturating_sub(since);
    if since > head || lag > i64::from(max) {
        return Ok(Resume {
            after: head,
            reset: true,
        });
    }
    Ok(Resume {
        after: since,
        reset: false,
    })
}

fn last_event_id(headers: &HeaderMap) -> Result<Option<i64>, AppError> {
    headers
        .get("last-event-id")
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|text| text.trim().parse::<i64>().ok())
                .ok_or_else(|| {
                    AppError::invalid("invalid-last-event-id", "Last-Event-ID must be a number")
                })
        })
        .transpose()
}

fn change_event(change: &Change) -> Event {
    Event::default()
        .event("change")
        .id(change.seq.to_string())
        .json_data(ChangeDto::from(change))
        .unwrap_or_else(|_| reset_event())
}

fn reset_event() -> Event {
    Event::default().event("reset").data("{}")
}

struct Cursor {
    feed: Arc<dyn ChangeFeed>,
    wake: broadcast::Receiver<Change>,
    poll: Interval,
    after: i64,
    pending: VecDeque<Change>,
    reset: bool,
}

impl Cursor {
    async fn next(mut self) -> Option<(Event, Self)> {
        if std::mem::take(&mut self.reset) {
            return Some((reset_event(), self));
        }
        loop {
            if let Some(change) = self.pending.pop_front() {
                self.after = change.seq;
                return Some((change_event(&change), self));
            }
            match self.feed.changes_since(self.after, PAGE).await {
                Ok(changes) if !changes.is_empty() => self.pending.extend(changes),
                Ok(_) => self.sleep().await,
                Err(error) => {
                    tracing::warn!(error = %error, "event stream stopped");
                    return None;
                }
            }
        }
    }

    async fn sleep(&mut self) {
        tokio::select! {
            woken = self.wake.recv() => {
                if matches!(woken, Err(broadcast::error::RecvError::Closed)) {
                    self.poll.tick().await;
                }
            }
            _ = self.poll.tick() => {}
        }
    }
}

#[utoipa::path(
    get,
    path = "/events",
    tag = "events",
    params(EventsQuery),
    responses(
        (status = 200, content_type = "text/event-stream", body = ChangeDto,
            description = "`change` events in commit order, `reset` when the client must reload"),
        (status = 401, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn events(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<EventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let wake = state.events.subscribe();
    let since = last_event_id(&headers)?.or(query.since);
    let resume = plan(state.changes.as_ref(), since, MAX_BACKLOG).await?;
    let mut poll = tokio::time::interval(POLL);
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let cursor = Cursor {
        feed: state.changes.clone(),
        wake,
        poll,
        after: resume.after,
        pending: VecDeque::new(),
        reset: resume.reset,
    };
    let stream = stream::unfold(cursor, |cursor| async move {
        cursor
            .next()
            .await
            .map(|(event, cursor)| (Ok::<_, Infallible>(event), cursor))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::tests::{TestApi, bearer, test_api};

    async fn read_until(api: &TestApi, request: Request<Body>, needle: &str) -> String {
        let response = api.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200);
        let mut body = response.into_body();
        let mut text = String::new();
        while !text.contains(needle) {
            let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
                .await
                .expect("event in time")
                .unwrap()
                .unwrap();
            if let Ok(data) = frame.into_data() {
                text.push_str(&String::from_utf8_lossy(&data));
            }
        }
        text
    }

    #[tokio::test]
    async fn replays_changes_after_the_resume_point() {
        let api = test_api().await;
        api.projects.create("ARC", "Arches").await.unwrap();
        api.projects.create("BRD", "Bridge").await.unwrap();
        let request = bearer(Request::get("/api/v1/events?since=0"), &api.token)
            .body(Body::empty())
            .unwrap();
        let text = read_until(&api, request, "id: 2").await;
        assert!(text.contains("event: change"));
        assert!(text.contains("id: 1"));
        assert!(text.contains("\"entity\":\"project\""));
        assert!(text.contains("\"op\":\"insert\""));
    }

    #[tokio::test]
    async fn streams_new_changes_live() {
        let api = test_api().await;
        let request = bearer(Request::get("/api/v1/events"), &api.token)
            .header("last-event-id", "0")
            .body(Body::empty())
            .unwrap();
        let projects = api.projects.clone();
        let creator = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            projects.create("ARC", "Arches").await.unwrap();
        });
        let text = read_until(&api, request, "id: 1").await;
        assert!(text.contains("event: change"));
        creator.await.unwrap();
    }

    #[tokio::test]
    async fn plans_resets_for_clients_too_far_behind_or_ahead() {
        let api = test_api().await;
        for key in ["AA", "BB", "CC"] {
            api.projects.create(key, "Project").await.unwrap();
        }
        let feed = api.changes.as_ref();
        assert_eq!(
            plan(feed, None, 10).await.unwrap(),
            Resume {
                after: 3,
                reset: false
            }
        );
        assert_eq!(
            plan(feed, Some(1), 10).await.unwrap(),
            Resume {
                after: 1,
                reset: false
            }
        );
        assert_eq!(
            plan(feed, Some(0), 2).await.unwrap(),
            Resume {
                after: 3,
                reset: true
            }
        );
        assert_eq!(
            plan(feed, Some(9), 10).await.unwrap(),
            Resume {
                after: 3,
                reset: true
            }
        );
    }

    #[test]
    fn rejects_malformed_last_event_ids() {
        let mut headers = HeaderMap::new();
        assert_eq!(last_event_id(&headers).unwrap(), None);
        headers.insert("last-event-id", " 42 ".parse().unwrap());
        assert_eq!(last_event_id(&headers).unwrap(), Some(42));
        headers.insert("last-event-id", "nope".parse().unwrap());
        assert_eq!(
            last_event_id(&headers).unwrap_err().code(),
            "invalid-last-event-id"
        );
    }
}
