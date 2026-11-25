use axum::Json;
use axum::extract::State;
use nullradix_app::AppError;
use nullradix_app::dispatcher_prompts::{
    PreviewRequest, PromptList, PromptPreview, PromptState, PromptView,
};
use nullradix_domain::PromptKind;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

/// Kind of run a prompt template is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PromptKindDto {
    Work,
    Plan,
    Research,
    Merge,
}

impl PromptKindDto {
    /// The name used in routes and on the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Plan => "plan",
            Self::Research => "research",
            Self::Merge => "merge",
        }
    }
}

impl From<PromptKind> for PromptKindDto {
    fn from(kind: PromptKind) -> Self {
        match kind {
            PromptKind::Work => Self::Work,
            PromptKind::Plan => Self::Plan,
            PromptKind::Research => Self::Research,
            PromptKind::Merge => Self::Merge,
        }
    }
}

impl From<PromptKindDto> for PromptKind {
    fn from(kind: PromptKindDto) -> Self {
        match kind {
            PromptKindDto::Work => Self::Work,
            PromptKindDto::Plan => Self::Plan,
            PromptKindDto::Research => Self::Research,
            PromptKindDto::Merge => Self::Merge,
        }
    }
}

/// A placeholder a template may use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlaceholderDto {
    /// Used as `{{name}}`.
    pub name: String,
    /// What goes there.
    pub description: String,
}

/// A prompt kind with its default, its own template and what an editor needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptDto {
    pub kind: PromptKindDto,
    pub label: String,
    /// The template that applies without an own one.
    pub default: String,
    /// The own template; none means the default applies.
    pub template: Option<String>,
    pub placeholders: Vec<PlaceholderDto>,
    /// The parts every template of this kind must keep.
    pub required: Vec<String>,
}

impl From<PromptView> for PromptDto {
    fn from(view: PromptView) -> Self {
        Self {
            kind: view.kind.into(),
            label: view.label.to_owned(),
            default: view.default.to_owned(),
            template: view.template,
            placeholders: view
                .placeholders
                .into_iter()
                .map(|placeholder| PlaceholderDto {
                    name: placeholder.name.to_owned(),
                    description: placeholder.description.to_owned(),
                })
                .collect(),
            required: view.required.into_iter().map(str::to_owned).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptListDto {
    pub items: Vec<PromptDto>,
    /// Version of the dispatcher configuration the templates live in.
    pub version: i64,
}

impl From<PromptList> for PromptListDto {
    fn from(list: PromptList) -> Self {
        Self {
            items: list.items.into_iter().map(Into::into).collect(),
            version: list.version.value(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptDetail {
    pub prompt: PromptDto,
    /// Version of the dispatcher configuration the template lives in.
    pub version: i64,
}

impl From<PromptState> for PromptDetail {
    fn from(state: PromptState) -> Self {
        Self {
            prompt: state.prompt.into(),
            version: state.version.value(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetPrompt {
    /// The own template; blank goes back to the default.
    pub template: String,
    /// Version of the dispatcher configuration the change is based on.
    pub version: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct PromptPath {
    kind: PromptKindDto,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ResetPromptQuery {
    /// Version of the dispatcher configuration the change is based on.
    version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewPrompt {
    pub kind: PromptKindDto,
    /// Ticket key for work, plan and merge, question key for research; none uses sample values.
    pub key: Option<String>,
    /// Rendered instead of the stored template, without saving it.
    pub template: Option<String>,
}

/// An instruction as the agent would get it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptPreviewDto {
    pub kind: PromptKindDto,
    pub key: Option<String>,
    /// Sample values were used because no key was given.
    pub sample: bool,
    pub prompt: String,
    /// Placeholders that show a stand-in text because their data does not exist yet.
    pub stand_ins: Vec<String>,
}

impl From<PromptPreview> for PromptPreviewDto {
    fn from(preview: PromptPreview) -> Self {
        Self {
            kind: preview.kind.into(),
            key: preview.key,
            sample: preview.sample,
            prompt: preview.prompt,
            stand_ins: preview.stand_ins.into_iter().map(str::to_owned).collect(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/dispatcher/prompts",
    tag = "dispatcher",
    responses((status = 200, body = PromptListDto), (status = 401, body = Problem))
)]
pub(crate) async fn list_prompts(
    State(state): State<ApiState>,
) -> Result<Json<PromptListDto>, ApiError> {
    Ok(Json(state.dispatcher_prompts.list().await?.into()))
}

#[utoipa::path(
    get,
    path = "/dispatcher/prompts/{kind}",
    tag = "dispatcher",
    params(PromptPath),
    responses((status = 200, body = PromptDetail), (status = 401, body = Problem))
)]
pub(crate) async fn get_prompt(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<PromptPath>,
) -> Result<Json<PromptDetail>, ApiError> {
    Ok(Json(
        state.dispatcher_prompts.get(path.kind.into()).await?.into(),
    ))
}

#[utoipa::path(
    put,
    path = "/dispatcher/prompts/{kind}",
    tag = "dispatcher",
    params(PromptPath),
    request_body = SetPrompt,
    responses(
        (status = 200, body = PromptDetail),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "Unknown placeholders or missing parts: `dispatcher-prompt-invalid`", body = Problem),
    )
)]
pub(crate) async fn set_prompt(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<PromptPath>,
    ApiJson(input): ApiJson<SetPrompt>,
) -> Result<Json<PromptDetail>, ApiError> {
    let expected = AppError::client_version(input.version)?;
    let saved = state
        .dispatcher_prompts
        .set(path.kind.into(), &input.template, expected)
        .await?;
    Ok(Json(saved.into()))
}

#[utoipa::path(
    delete,
    path = "/dispatcher/prompts/{kind}",
    tag = "dispatcher",
    params(PromptPath, ResetPromptQuery),
    responses(
        (status = 200, description = "Dropped; the default is in charge again", body = PromptDetail),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn reset_prompt(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<PromptPath>,
    ApiQuery(query): ApiQuery<ResetPromptQuery>,
) -> Result<Json<PromptDetail>, ApiError> {
    let expected = AppError::client_version(query.version)?;
    let saved = state
        .dispatcher_prompts
        .reset(path.kind.into(), expected)
        .await?;
    Ok(Json(saved.into()))
}

#[utoipa::path(
    post,
    path = "/dispatcher/prompt-previews",
    tag = "dispatcher",
    request_body = PreviewPrompt,
    responses(
        (status = 200, body = PromptPreviewDto),
        (status = 401, body = Problem),
        (status = 404, description = "No such ticket or question", body = Problem),
        (status = 422, description = "Bad key, unknown placeholders or missing parts", body = Problem),
    )
)]
pub(crate) async fn preview_prompt(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<PreviewPrompt>,
) -> Result<Json<PromptPreviewDto>, ApiError> {
    let preview = state
        .dispatcher_prompts
        .preview(&PreviewRequest {
            kind: input.kind.into(),
            key: input.key,
            template: input.template,
        })
        .await?;
    Ok(Json(preview.into()))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;

    use crate::tests::{TestApi, bearer, send, test_api};

    async fn call(
        api: &TestApi,
        request: axum::http::request::Builder,
        body: Option<serde_json::Value>,
    ) -> (u16, serde_json::Value) {
        let request = bearer(request, &api.token);
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    const OWN_PLAN: &str =
        "Plan {{ticketId}} as {{agent}}. Save plan.json: nullradix plan import {{ticketId}}";

    #[tokio::test]
    async fn lists_the_kinds_with_defaults_placeholders_and_required_parts() {
        let api = test_api().await;
        let (status, list) = call(&api, Request::get("/api/v1/dispatcher/prompts"), None).await;
        assert_eq!(status, 200);
        assert_eq!(list["version"], 1);
        let kinds: Vec<&str> = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["work", "plan", "research", "merge"]);
        let work = &list["items"][0];
        assert_eq!(work["label"], "Work");
        assert!(work["default"].as_str().unwrap().contains("{{ticketId}}"));
        assert_eq!(work["template"], serde_json::Value::Null);
        assert_eq!(work["placeholders"][0]["name"], "agent");
        assert_eq!(work["required"].as_array().unwrap().len(), 6);
    }

    #[tokio::test]
    async fn sets_and_resets_an_own_template() {
        let api = test_api().await;
        let (status, saved) = call(
            &api,
            Request::put("/api/v1/dispatcher/prompts/plan"),
            Some(json!({ "template": OWN_PLAN, "version": 1 })),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(saved["version"], 2);
        assert_eq!(saved["prompt"]["template"], OWN_PLAN);
        let (_, shown) = call(&api, Request::get("/api/v1/dispatcher/prompts/plan"), None).await;
        assert_eq!(shown["prompt"]["template"], OWN_PLAN);
        let (_, config) = call(&api, Request::get("/api/v1/dispatcher/config"), None).await;
        assert_eq!(config["config"]["prompts"]["plan"], OWN_PLAN);

        let (status, stale) = call(
            &api,
            Request::delete("/api/v1/dispatcher/prompts/plan?version=1"),
            None,
        )
        .await;
        assert_eq!(
            (status, stale["code"].as_str()),
            (409, Some("version-conflict"))
        );
        let (status, reset) = call(
            &api,
            Request::delete("/api/v1/dispatcher/prompts/plan?version=2"),
            None,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(reset["version"], 3);
        assert_eq!(reset["prompt"]["template"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn refuses_a_template_with_unknown_placeholders_and_missing_parts() {
        let api = test_api().await;
        let (status, problem) = call(
            &api,
            Request::put("/api/v1/dispatcher/prompts/merge"),
            Some(json!({ "template": "Merge {{branch}} {{nope}}", "version": 1 })),
        )
        .await;
        assert_eq!(status, 422);
        assert_eq!(problem["code"], "dispatcher-prompt-invalid");
        assert_eq!(problem["errors"][0]["field"], "template");
        assert_eq!(
            problem["errors"][0]["message"],
            "uses unknown placeholders {{nope}}"
        );
        assert!(
            problem["errors"][1]["message"]
                .as_str()
                .unwrap()
                .starts_with("lacks required parts:")
        );
    }

    #[tokio::test]
    async fn refuses_an_unknown_kind_and_malformed_bodies() {
        let api = test_api().await;
        let (status, _) = call(&api, Request::get("/api/v1/dispatcher/prompts/nope"), None).await;
        assert_eq!(status, 422);
        let (status, _) = call(
            &api,
            Request::put("/api/v1/dispatcher/prompts/plan"),
            Some(json!({ "template": OWN_PLAN })),
        )
        .await;
        assert_eq!(status, 422);
    }

    #[tokio::test]
    async fn previews_sample_values_and_a_draft_template() {
        let api = test_api().await;
        let (status, sample) = call(
            &api,
            Request::post("/api/v1/dispatcher/prompt-previews"),
            Some(json!({ "kind": "work" })),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(sample["sample"], true);
        assert_eq!(sample["key"], serde_json::Value::Null);
        assert!(
            sample["prompt"]
                .as_str()
                .unwrap()
                .starts_with("You are worker-ARC-0042")
        );
        let (status, draft) = call(
            &api,
            Request::post("/api/v1/dispatcher/prompt-previews"),
            Some(json!({ "kind": "plan", "template": OWN_PLAN })),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            draft["prompt"],
            "Plan ARC-0042 as planner-ARC-0042. Save plan.json: nullradix plan import ARC-0042"
        );
        let (status, missing) = call(
            &api,
            Request::post("/api/v1/dispatcher/prompt-previews"),
            Some(json!({ "kind": "work", "key": "ARC-0099" })),
        )
        .await;
        assert_eq!(
            (status, missing["code"].as_str()),
            (404, Some("ticket-not-found"))
        );
    }
}
