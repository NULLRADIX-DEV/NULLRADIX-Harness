//! Customizations: the skills and output styles a person replaced or added.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use nullradix_app::AppError;
use nullradix_app::customizations::{Effective, OverlayLine, Source};
use nullradix_domain::{BaseState, ExtensionPoint, Finding, OverlayKind, OverlayState};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{API_PREFIX, ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ExtensionPointDto {
    Skill,
    OutputStyle,
}

impl From<ExtensionPoint> for ExtensionPointDto {
    fn from(point: ExtensionPoint) -> Self {
        match point {
            ExtensionPoint::Skill => Self::Skill,
            ExtensionPoint::OutputStyle => Self::OutputStyle,
        }
    }
}

impl From<ExtensionPointDto> for ExtensionPoint {
    fn from(point: ExtensionPointDto) -> Self {
        match point {
            ExtensionPointDto::Skill => Self::Skill,
            ExtensionPointDto::OutputStyle => Self::OutputStyle,
        }
    }
}

/// Replaces a shipped file or comes on top of the shipped ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayKindDto {
    Replace,
    New,
}

impl From<OverlayKind> for OverlayKindDto {
    fn from(kind: OverlayKind) -> Self {
        match kind {
            OverlayKind::Replace => Self::Replace,
            OverlayKind::New => Self::New,
        }
    }
}

/// Whether the customization applies: problems send it back to the shipped file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayStateDto {
    Active,
    Fallback,
    Disabled,
}

impl From<OverlayState> for OverlayStateDto {
    fn from(state: OverlayState) -> Self {
        match state {
            OverlayState::Active => Self::Active,
            OverlayState::Fallback => Self::Fallback,
            OverlayState::Disabled => Self::Disabled,
        }
    }
}

/// How the starting point of a replacement compares with the shipped file today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum BaseStateDto {
    None,
    Current,
    Outdated,
    Unknown,
}

impl From<BaseState> for BaseStateDto {
    fn from(state: BaseState) -> Self {
        match state {
            BaseState::None => Self::None,
            BaseState::Current => Self::Current,
            BaseState::Outdated => Self::Outdated,
            BaseState::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FindingDto {
    pub code: String,
    pub message: String,
}

impl From<&Finding> for FindingDto {
    fn from(finding: &Finding) -> Self {
        Self {
            code: finding.code.to_owned(),
            message: finding.message.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CustomizationDto {
    pub point: ExtensionPointDto,
    pub name: String,
    pub kind: OverlayKindDto,
    pub state: OverlayStateDto,
    pub content: String,
    pub enabled: bool,
    pub base: BaseStateDto,
    /// Whether a shipped file of that name exists.
    pub standard: bool,
    /// Reasons the shipped file stays in charge.
    pub problems: Vec<FindingDto>,
    /// Hints that leave the customization in charge.
    pub warnings: Vec<FindingDto>,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: i64,
}

impl From<&OverlayLine> for CustomizationDto {
    fn from(line: &OverlayLine) -> Self {
        let record = line.overlay.record();
        Self {
            point: record.point.into(),
            name: record.name.to_string(),
            kind: record.kind.into(),
            state: line.state.into(),
            content: record.content.clone(),
            enabled: record.enabled,
            base: line.base.into(),
            standard: line.standard,
            problems: line.problems.iter().map(FindingDto::from).collect(),
            warnings: line.warnings.iter().map(FindingDto::from).collect(),
            created_at: record.created_at.as_millis(),
            updated_at: record.updated_at.as_millis(),
            version: record.version.value(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CustomizationList {
    /// Skills first, then output styles, each by name.
    pub items: Vec<CustomizationDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateCustomization {
    pub point: ExtensionPointDto,
    /// Lowercase words joined by single hyphens, at most 64 characters.
    pub name: String,
    /// The whole file with its head; left out copies the shipped file of that name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateCustomization {
    /// Replaces the content; left out keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Switches the customization on or off; left out keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Version the change is based on; a newer stored version fails with `version-conflict`.
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RebaseCustomization {
    pub version: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SourceDto {
    Custom,
    Standard,
}

/// The text that is in charge of a name right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EffectiveDto {
    pub source: SourceDto,
    pub content: String,
    /// Why the customization does not apply, when the shipped file took over.
    pub problems: Vec<FindingDto>,
}

impl From<&Effective> for EffectiveDto {
    fn from(effective: &Effective) -> Self {
        Self {
            source: match effective.source {
                Source::Custom => SourceDto::Custom,
                Source::Standard => SourceDto::Standard,
            },
            content: effective.content.clone(),
            problems: effective.problems.iter().map(FindingDto::from).collect(),
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct CustomizationPath {
    /// Where the customization takes effect.
    point: ExtensionPointDto,
    /// Name of the skill or output style.
    name: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct DeleteCustomizationQuery {
    /// Version the deletion is based on.
    version: i64,
}

#[utoipa::path(
    get,
    path = "/customizations",
    tag = "customizations",
    responses((status = 200, body = CustomizationList), (status = 401, body = Problem))
)]
pub(crate) async fn list_customizations(
    State(state): State<ApiState>,
) -> Result<Json<CustomizationList>, ApiError> {
    let lines = state.customizations.list().await?;
    Ok(Json(CustomizationList {
        items: lines.iter().map(CustomizationDto::from).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/customizations",
    tag = "customizations",
    request_body = CreateCustomization,
    responses(
        (status = 201, body = CustomizationDto,
            headers(("Location" = String, description = "URL of the customization"))),
        (status = 401, body = Problem),
        (status = 409, description = "Already customized: `customization-exists`", body = Problem),
        (status = 422, description = "Bad name or content, a head key that is not allowed (`customization-head-refused`, with the keys as errors), or nothing shipped to copy: `customization-no-standard`", body = Problem),
    )
)]
pub(crate) async fn create_customization(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<CreateCustomization>,
) -> Result<(StatusCode, HeaderMap, Json<CustomizationDto>), ApiError> {
    let line = state
        .customizations
        .create(input.point.into(), &input.name, input.content.as_deref())
        .await?;
    let dto = CustomizationDto::from(&line);
    let mut headers = HeaderMap::new();
    let point = ExtensionPoint::from(input.point).as_str();
    let location = format!("{API_PREFIX}/customizations/{point}/{}", dto.name);
    if let Ok(value) = HeaderValue::from_str(&location) {
        headers.insert(header::LOCATION, value);
    }
    Ok((StatusCode::CREATED, headers, Json(dto)))
}

#[utoipa::path(
    get,
    path = "/customizations/{point}/{name}",
    tag = "customizations",
    params(CustomizationPath),
    responses(
        (status = 200, body = CustomizationDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn get_customization(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<CustomizationPath>,
) -> Result<Json<CustomizationDto>, ApiError> {
    let line = state
        .customizations
        .get(path.point.into(), &path.name)
        .await?;
    Ok(Json(CustomizationDto::from(&line)))
}

#[utoipa::path(
    patch,
    path = "/customizations/{point}/{name}",
    tag = "customizations",
    params(CustomizationPath),
    request_body = UpdateCustomization,
    responses(
        (status = 200, body = CustomizationDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "Bad content or a head key that is not allowed: `customization-head-refused`", body = Problem),
    )
)]
pub(crate) async fn update_customization(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<CustomizationPath>,
    ApiJson(input): ApiJson<UpdateCustomization>,
) -> Result<Json<CustomizationDto>, ApiError> {
    let line = state
        .customizations
        .update(
            path.point.into(),
            &path.name,
            input.content.as_deref(),
            input.enabled,
            AppError::client_version(input.version)?,
        )
        .await?;
    Ok(Json(CustomizationDto::from(&line)))
}

#[utoipa::path(
    delete,
    path = "/customizations/{point}/{name}",
    tag = "customizations",
    params(CustomizationPath, DeleteCustomizationQuery),
    responses(
        (status = 204, description = "Dropped; the shipped file is in charge again"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn delete_customization(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<CustomizationPath>,
    ApiQuery(query): ApiQuery<DeleteCustomizationQuery>,
) -> Result<StatusCode, ApiError> {
    state
        .customizations
        .delete(
            path.point.into(),
            &path.name,
            AppError::client_version(query.version)?,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/customizations/{point}/{name}/rebase",
    tag = "customizations",
    params(CustomizationPath),
    request_body = RebaseCustomization,
    responses(
        (status = 200, description = "The shipped file is the new starting point", body = CustomizationDto),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "Nothing shipped: `customization-no-standard`", body = Problem),
    )
)]
pub(crate) async fn rebase_customization(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<CustomizationPath>,
    ApiJson(input): ApiJson<RebaseCustomization>,
) -> Result<Json<CustomizationDto>, ApiError> {
    let line = state
        .customizations
        .rebase(
            path.point.into(),
            &path.name,
            AppError::client_version(input.version)?,
        )
        .await?;
    Ok(Json(CustomizationDto::from(&line)))
}

#[utoipa::path(
    get,
    path = "/customizations/{point}/{name}/effective",
    tag = "customizations",
    params(CustomizationPath),
    responses(
        (status = 200, description = "The customization when it applies, else the shipped file", body = EffectiveDto),
        (status = 401, body = Problem),
        (status = 404, description = "Neither applies", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn effective_customization(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<CustomizationPath>,
) -> Result<Json<EffectiveDto>, ApiError> {
    let effective = state
        .customizations
        .effective(path.point.into(), &path.name)
        .await?;
    Ok(Json(EffectiveDto::from(&effective)))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use serde_json::{Value, json};

    use crate::tests::{TestApi, bearer, local, send, test_api};

    const SKILL: &str = "---\nname: review\ndescription: Reviews a change\n---\nBody\n";

    async fn call(api: &TestApi, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let request = bearer(Request::builder().method(method).uri(path), &api.token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    #[allow(clippy::unnecessary_wraps, reason = "it is the body of a request")]
    fn create(point: &str, name: &str, content: &str) -> Option<Value> {
        Some(json!({ "point": point, "name": name, "content": content }))
    }

    #[tokio::test]
    async fn a_head_with_keys_that_run_code_is_refused_with_the_keys_as_fields() {
        let api = test_api().await;
        let hooks = "---\nname: review\ndescription: Reviews\nallowed-tools: Bash\nhooks:\n  Stop: []\n---\nBody\n";
        let (status, problem) = call(
            &api,
            "POST",
            "/api/v1/customizations",
            create("skill", "review", hooks),
        )
        .await;
        assert_eq!(status, 422, "{problem}");
        assert_eq!(problem["code"], "customization-head-refused");
        assert_eq!(
            problem["errors"],
            json!([
                { "field": "content", "message": "the head key allowed-tools is not allowed" },
                { "field": "content", "message": "the head key hooks is not allowed" },
            ])
        );
        let (_, list) = call(&api, "GET", "/api/v1/customizations", None).await;
        assert_eq!(list, json!({ "items": [] }));

        let (status, created) = call(
            &api,
            "POST",
            "/api/v1/customizations",
            create("skill", "review", SKILL),
        )
        .await;
        assert_eq!(status, 201, "{created}");
        let (status, problem) = call(
            &api,
            "PATCH",
            "/api/v1/customizations/skill/review",
            Some(json!({ "content": hooks, "version": 1 })),
        )
        .await;
        assert_eq!(status, 422, "{problem}");
        assert_eq!(problem["code"], "customization-head-refused");
        let (_, read) = call(&api, "GET", "/api/v1/customizations/skill/review", None).await;
        assert_eq!(
            (&read["content"], &read["version"]),
            (&json!(SKILL), &json!(1))
        );
    }

    #[tokio::test]
    async fn a_customization_is_created_read_changed_and_dropped() {
        let api = test_api().await;
        let (status, empty) = call(&api, "GET", "/api/v1/customizations", None).await;
        assert_eq!((status, &empty), (200, &json!({ "items": [] })));

        let (status, created) = call(
            &api,
            "POST",
            "/api/v1/customizations",
            create("skill", "review", SKILL),
        )
        .await;
        assert_eq!(status, 201, "{created}");
        assert_eq!(created["kind"], "new");
        assert_eq!(created["state"], "active");
        assert_eq!(created["base"], "none");
        assert_eq!(created["standard"], false);
        assert_eq!(created["version"], 1);

        let (status, read) = call(&api, "GET", "/api/v1/customizations/skill/review", None).await;
        assert_eq!((status, &read["content"]), (200, &json!(SKILL)));

        let (status, patched) = call(
            &api,
            "PATCH",
            "/api/v1/customizations/skill/review",
            Some(json!({ "enabled": false, "version": 1 })),
        )
        .await;
        assert_eq!(
            (status, &patched["state"], &patched["version"]),
            (200, &json!("disabled"), &json!(2))
        );
        let (status, problem) = call(
            &api,
            "PATCH",
            "/api/v1/customizations/skill/review",
            Some(json!({ "enabled": true, "version": 1 })),
        )
        .await;
        assert_eq!(
            (status, &problem["code"]),
            (409, &json!("version-conflict"))
        );

        let (status, problem) = call(
            &api,
            "DELETE",
            "/api/v1/customizations/skill/review?version=1",
            None,
        )
        .await;
        assert_eq!(
            (status, &problem["code"]),
            (409, &json!("version-conflict"))
        );
        let (status, _) = call(
            &api,
            "DELETE",
            "/api/v1/customizations/skill/review?version=2",
            None,
        )
        .await;
        assert_eq!(status, 204);
        let (status, problem) =
            call(&api, "GET", "/api/v1/customizations/skill/review", None).await;
        assert_eq!(
            (status, &problem["code"]),
            (404, &json!("customization-missing"))
        );
    }

    #[tokio::test]
    async fn problems_are_listed_and_the_effective_text_follows_them() {
        let api = test_api().await;
        let (_, created) = call(
            &api,
            "POST",
            "/api/v1/customizations",
            create("output-style", "terse", "no head"),
        )
        .await;
        assert_eq!(created["state"], "fallback");
        assert_eq!(created["problems"][0]["code"], "description-missing");
        let (status, problem) = call(
            &api,
            "GET",
            "/api/v1/customizations/output-style/terse/effective",
            None,
        )
        .await;
        assert_eq!(
            (status, &problem["code"]),
            (404, &json!("customization-inactive"))
        );

        let (_, fixed) = call(
            &api,
            "PATCH",
            "/api/v1/customizations/output-style/terse",
            Some(json!({ "content": "---\ndescription: Short\n---\nBe brief", "version": 1 })),
        )
        .await;
        assert_eq!(fixed["state"], "active");
        let (status, effective) = call(
            &api,
            "GET",
            "/api/v1/customizations/output-style/terse/effective",
            None,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(effective["source"], "custom");
        assert_eq!(
            effective["content"],
            "---\ndescription: Short\n---\nBe brief"
        );

        let (_, list) = call(&api, "GET", "/api/v1/customizations", None).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn bad_input_is_refused_with_stable_codes() {
        let api = test_api().await;
        let cases = [
            (
                create("skill", "Bad Name", SKILL),
                422,
                "customization-name-invalid",
            ),
            (
                create("skill", "review", "  "),
                422,
                "customization-content-empty",
            ),
            (
                Some(json!({ "point": "skill", "name": "review" })),
                422,
                "customization-no-standard",
            ),
            (
                Some(json!({ "point": "workflow", "name": "x", "content": "y" })),
                422,
                "input-invalid",
            ),
            (
                Some(json!({ "point": "skill", "name": "x", "content": "y", "extra": 1 })),
                422,
                "input-invalid",
            ),
        ];
        for (body, status, code) in cases {
            let (got, problem) = call(&api, "POST", "/api/v1/customizations", body.clone()).await;
            assert_eq!((got, &problem["code"]), (status, &json!(code)), "{body:?}");
        }
        let (status, problem) = call(&api, "GET", "/api/v1/customizations/workflow/x", None).await;
        assert_eq!((status, &problem["code"]), (422, &json!("path-invalid")));
        let (status, problem) = call(&api, "GET", "/api/v1/customizations/skill/Bad", None).await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("customization-name-invalid"))
        );
        let (status, problem) = call(
            &api,
            "POST",
            "/api/v1/customizations/skill/review/rebase",
            Some(json!({ "version": 1 })),
        )
        .await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("customization-no-standard"))
        );
        let (status, _) = call(
            &api,
            "PATCH",
            "/api/v1/customizations/skill/review",
            Some(json!({ "version": 0 })),
        )
        .await;
        assert_eq!(status, 422);
    }

    #[tokio::test]
    async fn the_same_name_is_taken_once_and_creation_says_where_it_is() {
        let api = test_api().await;
        let request = bearer(Request::post("/api/v1/customizations"), &api.token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                create("skill", "review", SKILL).unwrap().to_string(),
            ))
            .unwrap();
        let response = tower::ServiceExt::oneshot(api.router.clone(), request)
            .await
            .unwrap();
        assert_eq!(
            response.headers()[header::LOCATION],
            "/api/v1/customizations/skill/review"
        );
        let (status, problem) = call(
            &api,
            "POST",
            "/api/v1/customizations",
            create("skill", "review", SKILL),
        )
        .await;
        assert_eq!(
            (status, &problem["code"]),
            (409, &json!("customization-exists"))
        );
        let (status, _) = call(
            &api,
            "POST",
            "/api/v1/customizations",
            create("output-style", "review", SKILL),
        )
        .await;
        assert_eq!(status, 201);
    }

    #[tokio::test]
    async fn customizations_need_a_login() {
        let api = test_api().await;
        for request in [
            local(Request::get("/api/v1/customizations")),
            local(Request::post("/api/v1/customizations")),
            local(Request::delete(
                "/api/v1/customizations/skill/review?version=1",
            )),
        ] {
            let (status, _, _) = send(&api.router, request.body(Body::empty()).unwrap()).await;
            assert_eq!(status, 401);
        }
    }
}
