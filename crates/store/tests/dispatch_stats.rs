#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::dispatch::local_time::UtcTime;
use nullradix_app::dispatch::stats::{
    DispatchStatsRepository, DispatchStatsService, StatsQuery, StatsRequest,
};
use nullradix_app::dispatcher_config::DispatcherConfigService;
use nullradix_app::projects::ProjectService;
use nullradix_app::tickets::{NewTicket, TicketService, WriteOptions};
use nullradix_app::{ChangeSignal, Clock, UuidV7Generator};
use nullradix_domain::{Board, Id, RunKind, SeriesUnit, Timestamp};
use nullradix_store::Store;
use uuid::Uuid;

struct ManualClock(AtomicI64);

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.0.load(Ordering::SeqCst))
    }
}

fn at(text: &str) -> i64 {
    Timestamp::parse_rfc3339(text).unwrap().as_millis()
}

/// Rows written beside the store: dispatch runs with the outcomes a test needs.
struct Raw(sqlx::SqlitePool);

struct Seed<'a> {
    subject: &'a str,
    kind: &'a str,
    profile: &'a str,
    at: &'a str,
    outcome: Option<&'a str>,
    category: Option<&'a str>,
}

impl Raw {
    async fn open(path: &std::path::Path) -> Self {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .busy_timeout(std::time::Duration::from_mins(1));
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        Self(pool)
    }

    async fn run(&self, seed: Seed<'_>) {
        self.priced(seed, None, None).await;
    }

    async fn priced(&self, seed: Seed<'_>, cost: Option<f64>, took_ms: Option<i64>) {
        let created = at(seed.at);
        sqlx::query(
            "INSERT INTO runs (id, label, status, created_at, started_at, finished_at, kind,
                               subject_key, profile_name, outcome, abort_category, cost_usd,
                               input_tokens, output_tokens)
             VALUES (?, ?, 'succeeded', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::now_v7())
        .bind(seed.subject)
        .bind(created)
        .bind(created)
        .bind(took_ms.map(|took| created + took))
        .bind(seed.kind)
        .bind(seed.subject)
        .bind(seed.profile)
        .bind(seed.outcome)
        .bind(seed.category)
        .bind(cost)
        .bind(cost.map(|_| 1000_i64))
        .bind(cost.map(|_| 100_i64))
        .execute(&self.0)
        .await
        .unwrap();
    }
}

struct Fixture {
    stats: DispatchStatsService,
    config: Arc<DispatcherConfigService>,
    store: Arc<Store>,
    clock: Arc<ManualClock>,
    raw: Raw,
    boards: [Board; 2],
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nullradix.db");
    let store = Arc::new(Store::open(&path).await.unwrap());
    let raw = Raw::open(&path).await;
    let signal = ChangeSignal::new();
    let clock = Arc::new(ManualClock(AtomicI64::new(at("2026-11-26T12:00:00Z"))));
    let ids = Arc::new(UuidV7Generator);
    let person = local_person(store.as_ref(), "anna", ids.as_ref(), clock.as_ref())
        .await
        .unwrap();
    let tickets = TicketService::new(
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    );
    let board_service = BoardService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        common::runners(&store, &signal).await,
        clock.clone(),
        ids.clone(),
        signal.clone(),
    );
    let project = ProjectService::new(store.clone(), clock.clone(), ids.clone(), signal.clone())
        .create("ARC", "Arches")
        .await
        .unwrap();
    let mut boards = Vec::new();
    for name in ["Main", "Side"] {
        boards.push(
            board_service
                .create(project.id(), name, &person)
                .await
                .unwrap()
                .board,
        );
    }
    // ARC-0001, 0002 and 0004 on Main, ARC-0003 on Side
    for (number, board) in [0, 0, 1, 0].into_iter().enumerate() {
        let board = &boards[board];
        tickets
            .create(
                board.project(),
                board.id(),
                &NewTicket {
                    title: format!("Work {number}"),
                    ..NewTicket::default()
                },
                &person,
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let config = Arc::new(DispatcherConfigService::new(
        store.clone(),
        store.clone(),
        clock.clone(),
        ids,
        signal,
    ));
    Fixture {
        stats: DispatchStatsService::new(
            store.clone(),
            config.clone(),
            store.clone(),
            clock.clone(),
            Arc::new(UtcTime),
        ),
        config,
        store,
        clock,
        raw,
        boards: [boards.remove(0), boards.remove(0)],
        _dir: dir,
    }
}

fn run<'a>(
    subject: &'a str,
    profile: &'a str,
    at: &'a str,
    outcome: &'a str,
    category: Option<&'a str>,
) -> Seed<'a> {
    Seed {
        subject,
        kind: "work",
        profile,
        at,
        outcome: Some(outcome),
        category,
    }
}

/// ARC-1 aborts twice, then done; ARC-2 done, then paused; ARC-3 fails; ARC-4 retried.
async fn seed(f: &Fixture) {
    let raw = &f.raw;
    let priced = run("ARC-0001", "large", "2026-11-24T12:00:00Z", "done", None);
    raw.run(run(
        "ARC-0001",
        "small",
        "2026-11-24T10:00:00Z",
        "abort",
        Some("max-turns"),
    ))
    .await;
    raw.run(run(
        "ARC-0001",
        "medium",
        "2026-11-24T11:00:00Z",
        "abort",
        Some("max-turns"),
    ))
    .await;
    raw.priced(priced, Some(2.5), Some(60_000)).await;
    raw.priced(
        run("ARC-0002", "small", "2026-11-25T09:00:00Z", "done", None),
        Some(0.5),
        Some(20_000),
    )
    .await;
    raw.run(run(
        "ARC-0002",
        "small",
        "2026-11-26T08:00:00Z",
        "pause-limit",
        Some("usage-limit"),
    ))
    .await;
    raw.run(run(
        "ARC-0003",
        "small",
        "2026-11-22T09:00:00Z",
        "abort-final",
        Some("exit-code"),
    ))
    .await;
    raw.run(run(
        "ARC-0004",
        "small",
        "2026-11-19T10:00:00Z",
        "abort",
        Some("timeout"),
    ))
    .await;
    raw.run(run(
        "ARC-0004",
        "medium",
        "2026-11-21T10:00:00Z",
        "done",
        None,
    ))
    .await;
    raw.run(Seed {
        kind: "merge",
        ..run(
            "ARC-0001",
            "medium",
            "2026-11-24T13:00:00Z",
            "merge-session",
            None,
        )
    })
    .await;
    raw.run(Seed {
        kind: "research",
        ..run(
            "ARC-Q0001",
            "small",
            "2026-11-23T09:00:00Z",
            "research",
            None,
        )
    })
    .await;
    raw.run(Seed {
        kind: "research",
        ..run(
            "ARC-Q0002",
            "small",
            "2026-11-23T10:00:00Z",
            "abort",
            Some("vanished"),
        )
    })
    .await;
    // a plain run of the journal has no dispatch data and never counts
    sqlx::query(
        "INSERT INTO runs (id, label, status, created_at) VALUES (?, 'plain', 'succeeded', ?)",
    )
    .bind(Uuid::now_v7())
    .bind(at("2026-11-24T09:00:00Z"))
    .execute(&f.raw.0)
    .await
    .unwrap();
}

fn week_request() -> StatsRequest<'static> {
    StatsRequest {
        from: Some("2026-11-20"),
        to: Some("2026-11-26"),
        ..StatsRequest::default()
    }
}

#[tokio::test]
async fn seeded_runs_give_the_kpis_the_ladder_and_the_abort_split() {
    let f = fixture().await;
    seed(&f).await;
    let report = f.stats.get(week_request()).await.unwrap();
    let stats = report.stats;
    assert_eq!(report.kind, RunKind::Work);
    let kpis = &stats.kpis;
    assert_eq!((kpis.runs, kpis.subjects), (7, 4));
    assert_eq!((kpis.done, kpis.aborted, kpis.final_failures), (3, 3, 1));
    assert_eq!(kpis.solved, 3);
    assert_eq!(kpis.success_rate, Some(0.75));
    assert_eq!(kpis.first_try_rate, Some(0.25));
    assert_eq!(kpis.escalation_rate, Some(0.5));
    assert_eq!(stats.ladder.finished, 4);
    assert_eq!(stats.ladder.after_escalation, Some(0.5));
    assert_eq!(stats.ladder.final_failures, Some(0.25));
    let reasons: Vec<_> = stats
        .abort_reasons
        .iter()
        .map(|reason| (reason.category.as_str(), reason.count))
        .collect();
    assert_eq!(reasons, [("max-turns", 2), ("exit-code", 1)]);
    assert!((stats.configurable_abort_share.unwrap() - 2.0 / 3.0).abs() < 1e-9);
    assert_eq!((stats.usage_pauses, stats.stale_runs), (1, 1));
    assert!((stats.usage.cost_usd - 3.0).abs() < 1e-9);
    assert_eq!(stats.usage.input_tokens, 2000);
    assert_eq!(stats.usage.output_tokens, 200);
    assert_eq!(stats.usage.average_duration_ms, Some(40_000));
    let profiles: Vec<_> = stats
        .by_profile
        .iter()
        .map(|row| (row.profile.as_str(), row.runs, row.done, row.escalated))
        .collect();
    assert_eq!(
        profiles,
        [("small", 4, 1, 1), ("medium", 2, 1, 1), ("large", 1, 1, 0)]
    );
    assert_eq!(stats.series.unit, SeriesUnit::Day);
    assert_eq!(stats.series.points.len(), 7);
    let points: Vec<_> = stats
        .series
        .points
        .iter()
        .map(|point| (point.start.as_str(), point.done, point.aborted, point.other))
        .collect();
    assert_eq!(points[1], ("2026-11-21", 1, 0, 0));
    assert_eq!(points[4], ("2026-11-24", 1, 2, 0));
    assert_eq!(points[6], ("2026-11-26", 0, 0, 0));
}

#[tokio::test]
async fn a_board_counts_the_runs_of_its_tickets_and_every_question_run() {
    let f = fixture().await;
    seed(&f).await;
    let main = f
        .stats
        .get(StatsRequest {
            board: Some(f.boards[0].id()),
            ..week_request()
        })
        .await
        .unwrap();
    assert_eq!(main.stats.kpis.runs, 6);
    assert_eq!(main.stats.kpis.final_failures, 0);
    assert_eq!(main.stats.kpis.subjects, 3);
    let side = f
        .stats
        .get(StatsRequest {
            board: Some(f.boards[1].id()),
            ..week_request()
        })
        .await
        .unwrap();
    assert_eq!(side.stats.kpis.runs, 1);
    assert_eq!(side.stats.kpis.final_failures, 1);
    let questions = f
        .stats
        .get(StatsRequest {
            board: Some(f.boards[1].id()),
            kind: Some("research"),
            ..week_request()
        })
        .await
        .unwrap();
    assert_eq!(questions.stats.kpis.runs, 2);
    let unknown: Id<Board> = Id::from_uuid(Uuid::now_v7());
    let missing = f
        .stats
        .get(StatsRequest {
            board: Some(unknown),
            ..week_request()
        })
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "board-not-found");
}

#[tokio::test]
async fn the_kind_picks_the_runs_and_aborts_split_by_it() {
    let f = fixture().await;
    seed(&f).await;
    let research = f
        .stats
        .get(StatsRequest {
            kind: Some("research"),
            ..week_request()
        })
        .await
        .unwrap()
        .stats;
    assert_eq!((research.kpis.runs, research.kpis.done), (2, 1));
    assert_eq!(research.kpis.aborted, 1);
    assert_eq!(research.abort_reasons[0].category, "vanished");
    let merge = f
        .stats
        .get(StatsRequest {
            kind: Some("merge"),
            ..week_request()
        })
        .await
        .unwrap()
        .stats;
    assert_eq!((merge.kpis.runs, merge.kpis.done), (1, 1));
    assert_eq!(merge.ladder.first_try, Some(1.0));
}

#[tokio::test]
async fn the_range_takes_whole_days_and_is_bounded_by_its_midnights() {
    let f = fixture().await;
    for (when, subject) in [
        ("2026-11-19T23:59:59.999Z", "ARC-0001"),
        ("2026-11-20T00:00:00Z", "ARC-0002"),
        ("2026-11-26T23:59:59.999Z", "ARC-0003"),
        ("2026-11-27T00:00:00Z", "ARC-0004"),
    ] {
        f.raw.run(run(subject, "small", when, "done", None)).await;
    }
    let stats = f.stats.get(week_request()).await.unwrap().stats;
    assert_eq!(stats.kpis.runs, 2);
    let found = f
        .store
        .runs(&StatsQuery {
            from: Timestamp::from_millis(at("2026-11-19T00:00:00Z")),
            until: Timestamp::from_millis(at("2026-11-28T00:00:00Z")),
            board: None,
            limit: 3,
        })
        .await
        .unwrap();
    let subjects: Vec<_> = found.iter().map(|run| run.subject.as_str()).collect();
    assert_eq!(subjects, ["ARC-0001", "ARC-0002", "ARC-0003"]);
}

#[tokio::test]
async fn without_input_the_range_is_56_days_by_week_and_bad_input_is_refused() {
    let f = fixture().await;
    let default = f.stats.get(StatsRequest::default()).await.unwrap();
    assert_eq!(default.stats.series.unit, SeriesUnit::Week);
    assert_eq!(default.range.days(), 56);
    for (request, code) in [
        (
            StatsRequest {
                from: Some("2026-11-27"),
                to: Some("2026-11-26"),
                ..StatsRequest::default()
            },
            "stats-range-invalid",
        ),
        (
            StatsRequest {
                from: Some("2025-01-01"),
                to: Some("2026-11-26"),
                ..StatsRequest::default()
            },
            "stats-range-invalid",
        ),
        (
            StatsRequest {
                from: Some("yesterday"),
                ..StatsRequest::default()
            },
            "stats-range-invalid",
        ),
        (
            StatsRequest {
                kind: Some("plan"),
                ..StatsRequest::default()
            },
            "stats-kind-invalid",
        ),
    ] {
        let error = f.stats.get(request).await.unwrap_err();
        assert_eq!(error.code(), code, "{request:?}");
    }
}

#[tokio::test]
async fn from_config_starts_on_the_day_the_newest_configuration_appeared() {
    let f = fixture().await;
    f.clock
        .0
        .store(at("2026-11-24T08:00:00Z"), Ordering::SeqCst);
    f.config.sync_hash().await.unwrap();
    f.clock
        .0
        .store(at("2026-11-26T12:00:00Z"), Ordering::SeqCst);
    f.raw
        .run(run(
            "ARC-0001",
            "small",
            "2026-11-23T10:00:00Z",
            "done",
            None,
        ))
        .await;
    f.raw
        .run(run(
            "ARC-0002",
            "small",
            "2026-11-24T10:00:00Z",
            "done",
            None,
        ))
        .await;
    let report = f
        .stats
        .get(StatsRequest {
            from: Some("config"),
            ..StatsRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(report.range.days(), 3);
    assert_eq!(
        report.config_changed_at,
        Some(Timestamp::from_millis(at("2026-11-24T08:00:00Z")))
    );
    assert_eq!(report.stats.kpis.runs, 1);
}

#[tokio::test]
async fn a_range_with_too_many_runs_is_refused_instead_of_cut() {
    let f = fixture().await;
    sqlx::query(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100001)
         INSERT INTO runs (id, label, status, created_at, kind, subject_key, profile_name)
         SELECT randomblob(16), 'x', 'succeeded', ?, 'work', 'ARC-0001', 'small' FROM n",
    )
    .bind(at("2026-11-25T10:00:00Z"))
    .execute(&f.raw.0)
    .await
    .unwrap();
    let error = f.stats.get(week_request()).await.unwrap_err();
    assert_eq!(error.code(), "stats-range-too-busy");
}

#[tokio::test]
async fn the_config_time_is_reported_for_any_range_up_to_its_end() {
    let f = fixture().await;
    f.clock
        .0
        .store(at("2026-11-24T08:00:00Z"), Ordering::SeqCst);
    f.config.sync_hash().await.unwrap();
    let seen = Some(Timestamp::from_millis(at("2026-11-24T08:00:00Z")));
    let default = f.stats.get(week_request()).await.unwrap();
    assert_eq!(default.config_changed_at, seen);
    assert_eq!(default.range.days(), 7);
    let before = f
        .stats
        .get(StatsRequest {
            from: Some("2026-11-10"),
            to: Some("2026-11-23"),
            ..StatsRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(before.config_changed_at, None);
}

#[tokio::test]
async fn an_old_abort_before_a_success_does_not_make_a_new_first_run_a_retry() {
    let f = fixture().await;
    f.raw
        .run(run(
            "ARC-0001",
            "small",
            "2026-06-01T10:00:00Z",
            "abort",
            Some("timeout"),
        ))
        .await;
    f.raw
        .run(run(
            "ARC-0001",
            "small",
            "2026-06-02T10:00:00Z",
            "done",
            None,
        ))
        .await;
    f.raw
        .run(run(
            "ARC-0001",
            "small",
            "2026-11-25T10:00:00Z",
            "done",
            None,
        ))
        .await;
    f.raw
        .run(run(
            "ARC-0002",
            "small",
            "2026-11-24T10:00:00Z",
            "abort",
            Some("timeout"),
        ))
        .await;
    f.raw
        .run(run(
            "ARC-0002",
            "small",
            "2026-11-25T10:00:00Z",
            "done",
            None,
        ))
        .await;
    let found = f
        .store
        .runs(&StatsQuery {
            from: Timestamp::from_millis(at("2026-11-20T00:00:00Z")),
            until: Timestamp::from_millis(at("2026-11-28T00:00:00Z")),
            board: None,
            limit: 10,
        })
        .await
        .unwrap();
    let retried: Vec<_> = found
        .iter()
        .map(|run| (run.subject.as_str(), run.retried))
        .collect();
    assert_eq!(
        retried,
        [("ARC-0002", false), ("ARC-0001", false), ("ARC-0002", true)]
    );
}

#[tokio::test]
async fn plan_runs_do_not_count_toward_the_run_cap() {
    let f = fixture().await;
    sqlx::query(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100001)
         INSERT INTO runs (id, label, status, created_at, kind, subject_key, profile_name)
         SELECT randomblob(16), 'x', 'succeeded', ?, 'plan', 'ARC-0001', 'small' FROM n",
    )
    .bind(at("2026-11-25T10:00:00Z"))
    .execute(&f.raw.0)
    .await
    .unwrap();
    let report = f.stats.get(week_request()).await.unwrap();
    assert_eq!(report.stats.kpis.runs, 0);
}
