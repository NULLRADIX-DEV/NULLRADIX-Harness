use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::{Stream, StreamExt, stream};
use nullradix_app::{AppError, Change, ChangeFeed, ChangeOp, Entity};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::time::{Interval, MissedTickBehavior};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{ApiError, ApiQuery, ApiState, Problem};

/// How far a client may lag behind before it must reload instead of replaying.
const MAX_BACKLOG: u32 = 1000;
const PAGE: u32 = 200;
/// Safety net for a missed wake-up; the signal normally wakes streams at once.
const POLL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EntityDto {
    Project,
    Board,
    Ticket,
    Run,
    Runner,
    Repository,
    Worktree,
    Comment,
    Note,
    Question,
    Session,
    InboxOrder,
    Favorite,
    Account,
    Theme,
    Setting,
    Customization,
    Snippet,
    DispatcherConfig,
    DispatcherState,
}

impl From<Entity> for EntityDto {
    fn from(entity: Entity) -> Self {
        match entity {
            Entity::Project => Self::Project,
            Entity::Board => Self::Board,
            Entity::Ticket => Self::Ticket,
            Entity::Run => Self::Run,
            Entity::Runner => Self::Runner,
            Entity::Repository => Self::Repository,
            Entity::Worktree => Self::Worktree,
            Entity::Comment => Self::Comment,
            Entity::Note => Self::Note,
            Entity::Question => Self::Question,
            Entity::Session => Self::Session,
            Entity::InboxOrder => Self::InboxOrder,
            Entity::Favorite => Self::Favorite,
            Entity::Account => Self::Account,
            Entity::Theme => Self::Theme,
            Entity::Setting => Self::Setting,
            Entity::Customization => Self::Customization,
            Entity::Snippet => Self::Snippet,
            Entity::DispatcherConfig => Self::DispatcherConfig,
            Entity::DispatcherState => Self::DispatcherState,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeOpDto {
    Insert,
    Update,
    Delete,
}

impl From<ChangeOp> for ChangeOpDto {
    fn from(op: ChangeOp) -> Self {
        match op {
            ChangeOp::Insert => Self::Insert,
            ChangeOp::Update => Self::Update,
            ChangeOp::Delete => Self::Delete,
        }
    }
}

/// Data of a `change` event; the event id carries `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeDto {
    pub seq: i64,
    pub entity: EntityDto,
    pub entity_id: Uuid,
    pub op: ChangeOpDto,
    pub at: i64,
}

impl From<&Change> for ChangeDto {
    fn from(change: &Change) -> Self {
        Self {
            seq: change.seq,
            entity: change.entity.into(),
            entity_id: change.entity_id,
            op: change.op.into(),
            at: change.at.as_millis(),
        }
    }
}

/// Data of the `ready` and `reset` events: the position the stream continues from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StreamPosition {
    pub seq: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
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
                    AppError::invalid("last-event-id-invalid", "Last-Event-ID must be a number")
                })
        })
        .transpose()
}

fn position_event(name: &'static str, seq: i64) -> Event {
    Event::default()
        .event(name)
        .id(seq.to_string())
        .json_data(StreamPosition { seq })
        .unwrap_or_else(|_| Event::default().event(name).id(seq.to_string()))
}

fn change_event(change: &Change) -> Event {
    Event::default()
        .event("change")
        .id(change.seq.to_string())
        .json_data(ChangeDto::from(change))
        .unwrap_or_else(|_| position_event("reset", change.seq))
}

struct Cursor {
    feed: Arc<dyn ChangeFeed>,
    head: watch::Receiver<i64>,
    poll: Interval,
    after: i64,
    pending: VecDeque<Change>,
}

impl Cursor {
    async fn next(mut self) -> Option<(Event, Self)> {
        loop {
            if let Some(change) = self.pending.pop_front() {
                self.after = change.seq;
                return Some((change_event(&change), self));
            }
            match self.feed.changes_since(self.after, PAGE).await {
                Ok(changes) if !changes.is_empty() => self.pending.extend(changes),
                Ok(_) => self.wait().await,
                Err(error) => {
                    tracing::warn!(error = %error, "event stream stopped");
                    return None;
                }
            }
        }
    }

    /// Returns once the head moved past the cursor or the safety poll fired.
    async fn wait(&mut self) {
        if *self.head.borrow_and_update() > self.after {
            return;
        }
        tokio::select! {
            changed = self.head.changed() => {
                if changed.is_err() {
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
            description = "Server-sent events. The stream starts with `ready` (data `StreamPosition`). \
                `change` events (data `ChangeDto`) follow in commit order. `reset` (data `StreamPosition`) \
                means the client must drop its cached data. Every event id is a sequence number for \
                `Last-Event-ID`."),
        (status = 401, body = Problem),
        (status = 422, body = Problem),
        (status = 503, description = "Too many open streams: `event-streams-busy`", body = Problem),
    )
)]
pub(crate) async fn events(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<EventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let slot = state.limits.event_stream().ok_or_else(|| {
        AppError::unavailable("event-streams-busy", "too many event streams are open")
    })?;
    let head = state.signal.subscribe();
    let since = last_event_id(&headers)?.or(query.since);
    let resume = plan(state.changes.as_ref(), since, MAX_BACKLOG).await?;
    let mut poll = tokio::time::interval(POLL);
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let cursor = Cursor {
        feed: state.changes.clone(),
        head,
        poll,
        after: resume.after,
        pending: VecDeque::new(),
    };
    let opening = if resume.reset {
        position_event("reset", resume.after)
    } else {
        position_event("ready", resume.after)
    };
    let changes = stream::unfold(cursor, Cursor::next);
    let shutdown = state.shutdown.clone().cancelled_owned();
    let events = stream::once(async move { opening })
        .chain(changes)
        .take_until(shutdown)
        .map(move |event| {
            // the place in the stream cap is held until the stream is dropped
            let _slot = &slot;
            Ok::<_, Infallible>(event)
        });
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::Limits;
    use crate::tests::{TestApi, bearer, local, send, test_api, test_api_with};

    async fn open(api: &TestApi, path: &str, last_event_id: Option<&str>) -> Body {
        let mut request = bearer(Request::get(path), &api.token);
        if let Some(id) = last_event_id {
            request = request.header("last-event-id", id);
        }
        let response = api
            .router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        response.into_body()
    }

    async fn read_until(body: &mut Body, text: &mut String, needle: &str) {
        while !text.contains(needle) {
            let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
                .await
                .expect("event in time")
                .expect("stream still open")
                .unwrap();
            if let Ok(data) = frame.into_data() {
                text.push_str(&String::from_utf8_lossy(&data));
            }
        }
    }

    #[tokio::test]
    async fn starts_with_ready_at_the_head_when_no_position_is_given() {
        let api = test_api().await;
        api.projects.create("ARC", "Arches").await.unwrap();
        let mut body = open(&api, "/api/v1/events", None).await;
        let mut text = String::new();
        read_until(&mut body, &mut text, "event: ready").await;
        // the local runner took seq 1
        assert!(text.contains("id: 2"));
        assert!(text.contains("{\"seq\":2}"));
    }

    #[tokio::test]
    async fn replays_changes_after_the_resume_point() {
        let api = test_api().await;
        api.projects.create("ARC", "Arches").await.unwrap();
        api.projects.create("BRD", "Bridge").await.unwrap();
        let mut body = open(&api, "/api/v1/events?since=0", None).await;
        let mut text = String::new();
        read_until(&mut body, &mut text, "id: 2").await;
        let ready = text.find("event: ready").unwrap();
        let first = text.find("event: change").unwrap();
        assert!(ready < first);
        assert!(text.contains("\"entity\":\"project\""));
        assert!(text.contains("\"op\":\"insert\""));
    }

    #[tokio::test]
    async fn streams_new_changes_live_in_order_without_gaps() {
        let api = test_api().await;
        let mut body = open(&api, "/api/v1/events", Some("0")).await;
        let projects = api.projects.clone();
        let creator = tokio::spawn(async move {
            for index in 0..300 {
                let key = format!("P{index}");
                projects.create(&key, "Burst").await.unwrap();
            }
        });
        let mut text = String::new();
        read_until(&mut body, &mut text, "id: 300\n").await;
        creator.await.unwrap();
        let ids: Vec<i64> = text
            .lines()
            .filter_map(|line| line.strip_prefix("id: "))
            .filter_map(|id| id.parse().ok())
            .filter(|id| *id > 0)
            .collect();
        assert_eq!(ids, (1..=300).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn tells_clients_ahead_of_the_feed_to_reset() {
        let api = test_api().await;
        let mut body = open(&api, "/api/v1/events", Some("99")).await;
        let mut text = String::new();
        read_until(&mut body, &mut text, "event: reset").await;
        assert!(text.contains("id: 1"));
    }

    #[tokio::test]
    async fn streams_end_when_the_daemon_shuts_down() {
        let api = test_api().await;
        let mut body = open(&api, "/api/v1/events", None).await;
        let mut text = String::new();
        read_until(&mut body, &mut text, "event: ready").await;
        api.shutdown.cancel();
        let end = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(frame) = body.frame().await {
                frame.unwrap();
            }
        })
        .await;
        assert!(end.is_ok(), "stream did not end");
    }

    #[tokio::test]
    async fn refuses_streams_above_the_cap_until_one_ends() {
        let limits = Limits::new(Duration::from_secs(10), Duration::from_secs(10), 8, 2);
        let api = test_api_with(None, limits).await;
        let first = open(&api, "/api/v1/events", None).await;
        let _second = open(&api, "/api/v1/events", None).await;
        let request = bearer(Request::get("/api/v1/events"), &api.token)
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, 503);
        assert_eq!(json["code"], "event-streams-busy");
        drop(first);
        let _third = open(&api, "/api/v1/events", None).await;
    }

    #[tokio::test]
    async fn needs_credentials() {
        let api = test_api().await;
        let request = local(Request::get("/api/v1/events"))
            .body(Body::empty())
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        assert_eq!(status, 401);
        assert_eq!(json["code"], "credentials-missing");
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
                after: 4,
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
                after: 4,
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
            "last-event-id-invalid"
        );
    }
}
