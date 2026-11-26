use axum::Json;
use axum::extract::State;
use nullradix_app::dispatch::stats::{StatsReport, StatsRequest};
use nullradix_domain::{
    AbortReason, DispatchStats, Id, OutcomeCount, ProfileRow, SeriesPoint, SeriesUnit, StatsKpis,
    StatsLadder, StatsSeries, StatsUsage, Timestamp, day_label,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::dispatch_runs::{DispatchOutcomeDto, RunKindDto};
use crate::{ApiError, ApiQuery, ApiState, Problem};

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatsQuery {
    /// First day as `YYYY-MM-DD`, or `config`; 56 days before `to` when left out.
    from: Option<String>,
    /// Last day as `YYYY-MM-DD` in local time; today when left out. A range has at most 366 days.
    to: Option<String>,
    /// `work` (default), `merge` or `research`.
    kind: Option<String>,
    /// Count the runs of this board's tickets only.
    board_id: Option<Uuid>,
}

/// The range and filters the numbers were worked out for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatsFilterDto {
    pub board_id: Option<Uuid>,
    /// First day, `YYYY-MM-DD`.
    pub from: String,
    /// Last day, `YYYY-MM-DD`.
    pub to: String,
    pub kind: RunKindDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatsKpisDto {
    pub runs: u32,
    /// Tickets or questions the runs were about.
    pub subjects: u32,
    pub done: u32,
    pub aborted: u32,
    /// Subjects with a successful run.
    pub solved: u32,
    /// Solved among the subjects that finished.
    pub success_rate: Option<f64>,
    /// Finished subjects solved by the first run, without an abort before.
    pub first_try_rate: Option<f64>,
    /// Subjects with a run after an abort.
    pub escalation_rate: Option<f64>,
    pub final_failures: u32,
}

impl From<StatsKpis> for StatsKpisDto {
    fn from(kpis: StatsKpis) -> Self {
        Self {
            runs: kpis.runs,
            subjects: kpis.subjects,
            done: kpis.done,
            aborted: kpis.aborted,
            solved: kpis.solved,
            success_rate: kpis.success_rate,
            first_try_rate: kpis.first_try_rate,
            escalation_rate: kpis.escalation_rate,
            final_failures: kpis.final_failures,
        }
    }
}

/// Cost, tokens and time of the runs counted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatsUsageDto {
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// Mean over the runs with a known duration.
    pub average_duration_ms: Option<i64>,
}

impl From<StatsUsage> for StatsUsageDto {
    fn from(usage: StatsUsage) -> Self {
        Self {
            cost_usd: usage.cost_usd,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            average_duration_ms: usage.average_duration_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SeriesUnitDto {
    Day,
    Week,
}

impl From<SeriesUnit> for SeriesUnitDto {
    fn from(unit: SeriesUnit) -> Self {
        match unit {
            SeriesUnit::Day => Self::Day,
            SeriesUnit::Week => Self::Week,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SeriesPointDto {
    /// First day of the bucket, `YYYY-MM-DD`; a Monday for weeks.
    pub start: String,
    pub done: u32,
    pub aborted: u32,
    /// Waiting for a person: question or blocked.
    pub other: u32,
}

impl From<SeriesPoint> for SeriesPointDto {
    fn from(point: SeriesPoint) -> Self {
        Self {
            start: point.start,
            done: point.done,
            aborted: point.aborted,
            other: point.other,
        }
    }
}

/// Runs over time: a point per day up to 28 days, per week beyond.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatsSeriesDto {
    pub unit: SeriesUnitDto,
    pub points: Vec<SeriesPointDto>,
}

impl From<StatsSeries> for StatsSeriesDto {
    fn from(series: StatsSeries) -> Self {
        Self {
            unit: series.unit.into(),
            points: series.points.into_iter().map(Into::into).collect(),
        }
    }
}

/// How the runs of one profile ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRowDto {
    pub profile: String,
    pub runs: u32,
    pub done: u32,
    /// Aborted and tried again.
    pub escalated: u32,
    #[serde(rename = "final")]
    pub final_failures: u32,
    /// Question, blocked and runs still open.
    pub other: u32,
}

impl From<ProfileRow> for ProfileRowDto {
    fn from(row: ProfileRow) -> Self {
        Self {
            profile: row.profile,
            runs: row.runs,
            done: row.done,
            escalated: row.escalated,
            final_failures: row.final_failures,
            other: row.other,
        }
    }
}

/// How the finished subjects ended, as shares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatsLadderDto {
    pub finished: u32,
    pub first_try: Option<f64>,
    pub after_escalation: Option<f64>,
    #[serde(rename = "final")]
    pub final_failures: Option<f64>,
}

impl From<StatsLadder> for StatsLadderDto {
    fn from(ladder: StatsLadder) -> Self {
        Self {
            finished: ladder.finished,
            first_try: ladder.first_try,
            after_escalation: ladder.after_escalation,
            final_failures: ladder.final_failures,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AbortReasonDto {
    /// An abort category, or `other` when the run recorded none.
    pub category: String,
    pub label: String,
    pub count: u32,
}

impl From<AbortReason> for AbortReasonDto {
    fn from(reason: AbortReason) -> Self {
        Self {
            category: reason.category,
            label: reason.label,
            count: reason.count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutcomeCountDto {
    pub outcome: DispatchOutcomeDto,
    pub count: u32,
}

impl From<OutcomeCount> for OutcomeCountDto {
    fn from(row: OutcomeCount) -> Self {
        Self {
            outcome: row.outcome.into(),
            count: row.count,
        }
    }
}

/// The dashboard numbers of the dispatcher over a range of days.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DispatchStatsDto {
    pub filter: StatsFilterDto,
    /// When the newest configuration in force up to the range end first appeared.
    pub config_changed_at: Option<i64>,
    pub kpis: StatsKpisDto,
    pub usage: StatsUsageDto,
    pub series: StatsSeriesDto,
    pub by_profile: Vec<ProfileRowDto>,
    pub ladder: StatsLadderDto,
    /// Aborts by category, the most frequent first.
    pub abort_reasons: Vec<AbortReasonDto>,
    /// Share of the aborts caused by turn or time limits or a full context.
    pub configurable_abort_share: Option<f64>,
    /// Every outcome of the runs counted, the most frequent first.
    pub outcomes: Vec<OutcomeCountDto>,
    /// Pauses for a usage limit in the range, runs of every kind.
    pub usage_pauses: u32,
    /// Runs the worker lost in the range, runs of every kind.
    pub stale_runs: u32,
}

impl From<StatsReport> for DispatchStatsDto {
    fn from(report: StatsReport) -> Self {
        let DispatchStats {
            kind,
            kpis,
            usage,
            series,
            by_profile,
            ladder,
            abort_reasons,
            configurable_abort_share,
            outcomes,
            usage_pauses,
            stale_runs,
        } = report.stats;
        Self {
            filter: StatsFilterDto {
                board_id: report.board.as_ref().map(Id::as_uuid),
                from: day_label(report.range.first_day()),
                to: day_label(report.range.last_day()),
                kind: kind.into(),
            },
            config_changed_at: report.config_changed_at.map(Timestamp::as_millis),
            kpis: kpis.into(),
            usage: usage.into(),
            series: series.into(),
            by_profile: by_profile.into_iter().map(Into::into).collect(),
            ladder: ladder.into(),
            abort_reasons: abort_reasons.into_iter().map(Into::into).collect(),
            configurable_abort_share,
            outcomes: outcomes.into_iter().map(Into::into).collect(),
            usage_pauses,
            stale_runs,
        }
    }
}

#[utoipa::path(
    get,
    path = "/stats/dispatch",
    tag = "dispatcher",
    params(StatsQuery),
    responses(
        (status = 200, body = DispatchStatsDto),
        (status = 401, body = Problem),
        (status = 404, description = "Unknown board", body = Problem),
        (status = 422, description = "Bad range or kind, or too many runs in the range", body = Problem),
    )
)]
pub(crate) async fn get_dispatch_stats(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<StatsQuery>,
) -> Result<Json<DispatchStatsDto>, ApiError> {
    let report = state
        .dispatch_stats
        .get(StatsRequest {
            from: query.from.as_deref(),
            to: query.to.as_deref(),
            kind: query.kind.as_deref(),
            board: query.board_id.map(Id::from_uuid),
        })
        .await?;
    Ok(Json(report.into()))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;

    use crate::tests::{bearer, local, send, test_api};

    async fn get(path: &str, authorized: bool) -> (u16, serde_json::Value) {
        let api = test_api().await;
        let request = local(Request::get(format!("/api/v1/stats/dispatch{path}")));
        let request = if authorized {
            bearer(request, &api.token)
        } else {
            request
        };
        let (status, _, body) = send(&api.router, request.body(Body::empty()).unwrap()).await;
        (status.as_u16(), body)
    }

    #[tokio::test]
    async fn an_empty_range_has_zero_counts_no_rates_and_a_series_of_weeks() {
        let (status, body) = get("", true).await;
        assert_eq!(status, 200);
        assert_eq!(body["filter"]["kind"], "work");
        assert_eq!(body["filter"]["boardId"], json!(null));
        assert_eq!(body["series"]["unit"], "week");
        assert!(body["series"]["points"].as_array().unwrap().len() >= 8);
        assert_eq!(body["kpis"]["runs"], 0);
        assert_eq!(body["kpis"]["successRate"], json!(null));
        assert_eq!(
            body["ladder"],
            json!({ "finished": 0, "firstTry": null, "afterEscalation": null, "final": null })
        );
        assert_eq!(body["byProfile"], json!([]));
        assert_eq!(body["abortReasons"], json!([]));
        assert_eq!(body["outcomes"], json!([]));
        assert_eq!(body["usagePauses"], 0);
        assert_eq!(body["staleRuns"], 0);
        assert_eq!(body["configChangedAt"], json!(null));
    }

    #[tokio::test]
    async fn a_short_range_goes_by_day_and_names_its_days() {
        let (status, body) = get("?from=2026-11-20&to=2026-11-26&kind=merge", true).await;
        assert_eq!(status, 200);
        assert_eq!(body["filter"]["from"], "2026-11-20");
        assert_eq!(body["filter"]["to"], "2026-11-26");
        assert_eq!(body["filter"]["kind"], "merge");
        assert_eq!(body["series"]["unit"], "day");
        assert_eq!(body["series"]["points"].as_array().unwrap().len(), 7);
        assert_eq!(
            body["series"]["points"][0],
            json!({ "start": "2026-11-20", "done": 0, "aborted": 0, "other": 0 })
        );
    }

    #[tokio::test]
    async fn bad_input_is_a_problem_with_a_stable_code() {
        for (query, code) in [
            ("?from=2026-02-30", "stats-range-invalid"),
            ("?from=2026-11-27&to=2026-11-26", "stats-range-invalid"),
            ("?from=2025-01-01&to=2026-11-26", "stats-range-invalid"),
            ("?kind=plan", "stats-kind-invalid"),
        ] {
            let (status, body) = get(query, true).await;
            assert_eq!((status, &body["code"]), (422, &json!(code)), "{query}");
        }
        let (status, body) = get("?boardId=0190a0b0-0000-7000-8000-000000000001", true).await;
        assert_eq!((status, &body["code"]), (404, &json!("board-not-found")));
    }

    #[tokio::test]
    async fn the_route_needs_a_token() {
        let (status, _) = get("", false).await;
        assert_eq!(status, 401);
    }
}
