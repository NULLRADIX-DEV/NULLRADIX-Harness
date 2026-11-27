//! Themes: installed packages, the chosen theme and the files a theme serves.

use std::collections::BTreeMap;

use axum::Json;
use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nullradix_app::AppError;
use nullradix_app::themes::{ThemeList, ThemeSettings};
use nullradix_domain::{
    ColorScheme, InstalledTheme, MAX_PACKAGE_BYTES, STYLE_FILE, SettingSpec, SettingValue, Version,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{API_PREFIX, ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ColorSchemeDto {
    Light,
    Dark,
}

impl From<ColorScheme> for ColorSchemeDto {
    fn from(scheme: ColorScheme) -> Self {
        match scheme {
            ColorScheme::Light => Self::Light,
            ColorScheme::Dark => Self::Dark,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct InstallQuery {
    /// Confirmed: the package may bring code or network domains.
    #[serde(default)]
    confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ThemeOptionDto {
    pub value: String,
    pub label: String,
}

/// A setting the theme offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ThemeSettingDto {
    Boolean {
        key: String,
        label: String,
        hint: Option<String>,
        default: bool,
    },
    Select {
        key: String,
        label: String,
        hint: Option<String>,
        options: Vec<ThemeOptionDto>,
        default: String,
    },
}

/// An installed theme package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeDto {
    pub id: String,
    pub name: String,
    /// Version of the theme itself, a semver version.
    pub package_version: String,
    pub author: String,
    /// Range of SDK versions the theme runs with.
    pub sdk_version: String,
    pub color_scheme: ColorSchemeDto,
    pub start_color: Option<String>,
    /// URL of the preview image.
    pub preview_url: Option<String>,
    /// URL of the stylesheet the UI links while the theme is chosen.
    pub style_url: String,
    pub slots: Vec<String>,
    /// Domains the theme may load from.
    pub network: Vec<String>,
    pub settings: Vec<ThemeSettingDto>,
    /// Whether the package brings a theme for Claude Code.
    pub claude_theme: bool,
    pub installed_at: i64,
    pub updated_at: i64,
    /// Row version for deleting.
    pub version: i64,
}

fn file_url(id: &str, path: &str) -> String {
    format!("{API_PREFIX}/themes/{id}/files/{path}")
}

impl From<&InstalledTheme> for ThemeDto {
    fn from(theme: &InstalledTheme) -> Self {
        let record = theme.record();
        let manifest = record.manifest.record();
        let id = manifest.id.as_str();
        let settings = manifest
            .settings
            .iter()
            .flatten()
            .map(|(key, spec)| match spec {
                SettingSpec::Boolean {
                    label,
                    hint,
                    default,
                } => ThemeSettingDto::Boolean {
                    key: key.clone(),
                    label: label.clone(),
                    hint: hint.clone(),
                    default: *default,
                },
                SettingSpec::Select {
                    label,
                    hint,
                    options,
                    default,
                } => ThemeSettingDto::Select {
                    key: key.clone(),
                    label: label.clone(),
                    hint: hint.clone(),
                    options: options
                        .iter()
                        .map(|option| ThemeOptionDto {
                            value: option.value.clone(),
                            label: option.label.clone(),
                        })
                        .collect(),
                    default: default.clone(),
                },
            })
            .collect();
        Self {
            id: id.to_owned(),
            name: manifest.name.clone(),
            package_version: manifest.version.clone(),
            author: manifest.author.clone(),
            sdk_version: manifest.sdk_version.clone(),
            color_scheme: manifest.color_scheme.into(),
            start_color: manifest.start_color.clone(),
            preview_url: manifest
                .preview
                .as_deref()
                .map(|preview| file_url(id, preview)),
            style_url: format!("{}?v={}", file_url(id, STYLE_FILE), record.version.value()),
            slots: manifest.slots.clone(),
            network: manifest.network.clone(),
            settings,
            claude_theme: manifest.claude_theme,
            installed_at: record.installed_at.as_millis(),
            updated_at: record.updated_at.as_millis(),
            version: record.version.value(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ThemeListDto {
    /// Installed themes sorted by id.
    pub items: Vec<ThemeDto>,
    /// The chosen theme: `nullradix`, `system`, `light`, `dark` or an installed id.
    pub active: String,
}

impl From<&ThemeList> for ThemeListDto {
    fn from(list: &ThemeList) -> Self {
        Self {
            items: list.themes.iter().map(ThemeDto::from).collect(),
            active: list.active.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChooseTheme {
    /// A built-in theme or the id of an installed one.
    pub theme: String,
}

/// A switch or the value of one option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum ThemeSettingValueDto {
    Switch(bool),
    Choice(String),
}

impl From<ThemeSettingValueDto> for SettingValue {
    fn from(value: ThemeSettingValueDto) -> Self {
        match value {
            ThemeSettingValueDto::Switch(on) => Self::Switch(on),
            ThemeSettingValueDto::Choice(text) => Self::Choice(text),
        }
    }
}

impl From<&SettingValue> for ThemeSettingValueDto {
    fn from(value: &SettingValue) -> Self {
        match value {
            SettingValue::Switch(on) => Self::Switch(*on),
            SettingValue::Choice(text) => Self::Choice(text.clone()),
        }
    }
}

/// The values chosen for the settings of a theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSettingsDto {
    /// Chosen values by setting key; a key without a value uses the default of the manifest.
    pub values: BTreeMap<String, ThemeSettingValueDto>,
    /// Row version for saving and resetting; 0 while nothing was saved.
    pub version: i64,
}

impl From<&ThemeSettings> for ThemeSettingsDto {
    fn from(settings: &ThemeSettings) -> Self {
        Self {
            values: settings
                .values
                .values()
                .iter()
                .map(|(key, value)| (key.clone(), ThemeSettingValueDto::from(value)))
                .collect(),
            version: settings.version.map_or(0, Version::value),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveThemeSettings {
    /// All values of the theme; they replace the saved ones.
    pub values: BTreeMap<String, ThemeSettingValueDto>,
    /// Version the save is based on; 0 when nothing was saved yet.
    pub version: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ResetSettingsQuery {
    /// Version the reset is based on; 0 when nothing was saved.
    version: i64,
}

/// 0 means nothing saved; anything else is a version a client read.
fn expected_version(value: i64) -> Result<Option<Version>, AppError> {
    if value == 0 {
        return Ok(None);
    }
    AppError::client_version(value).map(Some)
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct RemoveThemeQuery {
    /// Version the removal is based on.
    version: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct ThemePath {
    /// Theme id.
    id: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct ThemeFilePath {
    /// Theme id.
    id: String,
    /// Path of the file inside the package.
    path: String,
}

#[utoipa::path(
    get,
    path = "/themes",
    tag = "themes",
    responses((status = 200, body = ThemeListDto), (status = 401, body = Problem))
)]
pub(crate) async fn list_themes(
    State(state): State<ApiState>,
) -> Result<Json<ThemeListDto>, ApiError> {
    Ok(Json(ThemeListDto::from(&state.themes.list().await?)))
}

#[utoipa::path(
    post,
    path = "/themes",
    tag = "themes",
    params(InstallQuery),
    request_body(
        content = Vec<u8>,
        content_type = "application/octet-stream",
        description = "A `.nullradix` theme package, a zip archive"
    ),
    responses(
        (status = 201, description = "Installed", body = ThemeDto,
            headers(("Location" = String, description = "URL of the stylesheet"))),
        (status = 200, description = "Replaced the installed theme with the same id", body = ThemeDto),
        (status = 401, body = Problem),
        (status = 409, description = "`theme-confirmation-required`: the package has code or network domains the installed version did not (retry with `confirmed=true` after the person confirmed them)", body = Problem),
        (status = 422, description = "Not a valid package: `theme-package-invalid`, `theme-manifest-invalid` with the refused fields, or `theme-id-reserved`", body = Problem),
    )
)]
pub(crate) async fn install_theme(
    State(state): State<ApiState>,
    ApiQuery(query): ApiQuery<InstallQuery>,
    body: Body,
) -> Result<(StatusCode, HeaderMap, Json<ThemeDto>), ApiError> {
    // the default body limit of extractors is far below a package, so the limit is set here
    let limit = usize::try_from(MAX_PACKAGE_BYTES).unwrap_or(usize::MAX);
    let archive = to_bytes(body, limit).await.map_err(|_| {
        AppError::invalid(
            "theme-package-invalid",
            format!("the package is larger than {MAX_PACKAGE_BYTES} bytes"),
        )
    })?;
    let theme = state
        .themes
        .install(archive.to_vec(), query.confirmed)
        .await?
        .theme;
    crate::claude_theme::resync(&state).await;
    let dto = ThemeDto::from(&theme);
    let created = theme.record().version.value() == 1;
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&dto.style_url) {
        headers.insert(header::LOCATION, value);
    }
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, headers, Json(dto)))
}

#[utoipa::path(
    put,
    path = "/settings/theme",
    tag = "themes",
    request_body = ChooseTheme,
    responses(
        (status = 200, body = ChooseTheme),
        (status = 401, body = Problem),
        (status = 404, description = "Not built in and not installed", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn choose_theme(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<ChooseTheme>,
) -> Result<Json<ChooseTheme>, ApiError> {
    let theme = state.themes.select(&input.theme).await?;
    crate::claude_theme::resync(&state).await;
    Ok(Json(ChooseTheme { theme }))
}

#[utoipa::path(
    delete,
    path = "/themes/{id}",
    tag = "themes",
    params(ThemePath, RemoveThemeQuery),
    responses(
        (status = 204, description = "Uninstalled; when it was chosen the default takes over"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn remove_theme(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ThemePath>,
    ApiQuery(query): ApiQuery<RemoveThemeQuery>,
) -> Result<StatusCode, ApiError> {
    state
        .themes
        .remove(&path.id, AppError::client_version(query.version)?)
        .await?;
    crate::claude_theme::resync(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/themes/{id}/settings",
    tag = "themes",
    params(ThemePath),
    responses(
        (status = 200, body = ThemeSettingsDto),
        (status = 401, body = Problem),
        (status = 404, description = "The theme is not installed", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn theme_settings(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ThemePath>,
) -> Result<Json<ThemeSettingsDto>, ApiError> {
    let settings = state.themes.settings(&path.id).await?;
    Ok(Json(ThemeSettingsDto::from(&settings)))
}

#[utoipa::path(
    put,
    path = "/themes/{id}/settings",
    tag = "themes",
    params(ThemePath),
    request_body = SaveThemeSettings,
    responses(
        (status = 200, body = ThemeSettingsDto),
        (status = 401, body = Problem),
        (status = 404, description = "The theme is not installed", body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, description = "`theme-settings-invalid` with the refused keys as fields", body = Problem),
    )
)]
pub(crate) async fn save_theme_settings(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ThemePath>,
    ApiJson(input): ApiJson<SaveThemeSettings>,
) -> Result<Json<ThemeSettingsDto>, ApiError> {
    let expected = expected_version(input.version)?;
    let values = input
        .values
        .into_iter()
        .map(|(key, value)| (key, SettingValue::from(value)))
        .collect();
    let saved = state
        .themes
        .save_settings(&path.id, values, expected)
        .await?;
    crate::claude_theme::resync(&state).await;
    Ok(Json(ThemeSettingsDto::from(&saved)))
}

#[utoipa::path(
    delete,
    path = "/themes/{id}/settings",
    tag = "themes",
    params(ThemePath, ResetSettingsQuery),
    responses(
        (status = 204, description = "Back to the defaults of the manifest, also when nothing was saved"),
        (status = 401, body = Problem),
        (status = 404, description = "The theme is not installed", body = Problem),
        (status = 409, description = "Stale version", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn reset_theme_settings(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ThemePath>,
    ApiQuery(query): ApiQuery<ResetSettingsQuery>,
) -> Result<StatusCode, ApiError> {
    let expected = expected_version(query.version)?;
    state.themes.reset_settings(&path.id, expected).await?;
    crate::claude_theme::resync(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/themes/{id}/files/{*path}",
    tag = "themes",
    params(ThemeFilePath),
    responses(
        (status = 200, description = "The file", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 401, body = Problem),
        (status = 404, description = "No such file inside the installed theme", body = Problem),
    )
)]
pub(crate) async fn theme_file(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<ThemeFilePath>,
) -> Result<Response, ApiError> {
    let asset = state.themes.asset(&path.id, &path.path).await?;
    Ok((
        [
            (header::CONTENT_TYPE, asset.content_type),
            // themes are replaced in place, so the browser asks again every time
            (header::CACHE_CONTROL, "no-cache"),
            // an svg or html file must not run script on the app origin
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:",
            ),
        ],
        asset.data,
    )
        .into_response())
}

#[cfg(test)]
pub(crate) mod fake {
    use std::collections::HashMap;

    use async_trait::async_trait;
    use nullradix_app::AppError;
    use nullradix_app::theme_package::PackageFile;
    use nullradix_app::themes::ThemeFiles;
    use nullradix_domain::ThemeId;
    use parking_lot::Mutex;

    /// Archives are `path=content` parts split by a `--` line; files stay in memory.
    #[derive(Default)]
    pub(crate) struct Files(Mutex<HashMap<String, Vec<PackageFile>>>);

    #[async_trait]
    impl ThemeFiles for Files {
        async fn unpack(&self, archive: Vec<u8>) -> Result<Vec<PackageFile>, AppError> {
            let text = String::from_utf8(archive)
                .map_err(|_| nullradix_app::theme_package::package_invalid("not an archive"))?;
            Ok(text
                .split("\n--\n")
                .filter_map(|part| part.split_once('='))
                .map(|(path, data)| PackageFile {
                    path: path.to_owned(),
                    data: data.as_bytes().to_vec(),
                })
                .collect())
        }

        async fn store(&self, id: &ThemeId, files: Vec<PackageFile>) -> Result<(), AppError> {
            self.0.lock().insert(id.to_string(), files);
            Ok(())
        }

        async fn remove(&self, id: &ThemeId) -> Result<(), AppError> {
            self.0.lock().remove(id.as_str());
            Ok(())
        }

        async fn read(&self, id: &ThemeId, path: &[String]) -> Result<Option<Vec<u8>>, AppError> {
            let joined = path.join("/");
            Ok(self
                .0
                .lock()
                .get(id.as_str())
                .and_then(|files| files.iter().find(|file| file.path == joined))
                .map(|file| file.data.clone()))
        }
        async fn paths(&self, id: &ThemeId) -> Result<Vec<String>, AppError> {
            let files = self.0.lock();
            let stored = files.get(id.as_str()).map_or(&[][..], Vec::as_slice);
            Ok(stored.iter().map(|file| file.path.clone()).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::tests::{TestApi, bearer, local, send, test_api};

    const MANIFEST: &str = r##"{
        "id": "night-blue", "name": "Night Blue", "version": "1.2.0", "author": "Ada",
        "sdkVersion": "^1.3", "colorScheme": "dark", "startColor": "#101018",
        "preview": "assets/preview.png", "slots": ["Progress"],
        "permissions": { "network": [] },
        "settingsSchema": { "dim": { "type": "boolean", "label": "Dim", "default": true } }
    }"##;

    fn package(manifest: &str) -> Vec<u8> {
        format!(
            "manifest.json={manifest}\n--\ntheme.css=:root {{ --accent: #6c63ff }}\n--\nassets/preview.png=PNG"
        )
        .into_bytes()
    }

    async fn call(api: &TestApi, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let request = bearer(Request::builder().method(method).uri(path), &api.token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        let (status, _, json) = send(&api.router, request).await;
        (status.as_u16(), json)
    }

    async fn upload(api: &TestApi, bytes: Vec<u8>) -> (u16, Option<String>, Value) {
        upload_to(api, "/api/v1/themes", bytes).await
    }

    async fn upload_to(api: &TestApi, path: &str, bytes: Vec<u8>) -> (u16, Option<String>, Value) {
        let request = bearer(Request::post(path), &api.token)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(bytes))
            .unwrap();
        let response = api.router.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get(header::LOCATION)
            .map(|value| value.to_str().unwrap().to_owned());
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        (
            status,
            location,
            serde_json::from_slice(&body).unwrap_or(Value::Null),
        )
    }

    struct Reply {
        status: u16,
        kind: Option<String>,
        cache: Option<String>,
        csp: Option<String>,
        body: Vec<u8>,
    }

    async fn file(api: &TestApi, path: &str) -> Reply {
        let request = bearer(Request::get(path), &api.token)
            .body(Body::empty())
            .unwrap();
        let response = api.router.clone().oneshot(request).await.unwrap();
        let text = |name| {
            response
                .headers()
                .get(name)
                .map(|value| value.to_str().unwrap().to_owned())
        };
        let (status, kind, cache, csp) = (
            response.status().as_u16(),
            text(header::CONTENT_TYPE),
            text(header::CACHE_CONTROL),
            text(header::CONTENT_SECURITY_POLICY),
        );
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        Reply {
            status,
            kind,
            cache,
            csp,
            body: body.to_vec(),
        }
    }

    #[tokio::test]
    async fn the_default_theme_is_chosen_until_another_is() {
        let api = test_api().await;
        let (status, list) = call(&api, "GET", "/api/v1/themes", None).await;
        assert_eq!(
            (status, &list),
            (200, &json!({ "items": [], "active": "nullradix" }))
        );
        let body = Some(json!({ "theme": "light" }));
        let (status, chosen) = call(&api, "PUT", "/api/v1/settings/theme", body).await;
        assert_eq!((status, &chosen["theme"]), (200, &json!("light")));
        let (_, list) = call(&api, "GET", "/api/v1/themes", None).await;
        assert_eq!(list["active"], "light");
    }

    #[tokio::test]
    async fn a_package_is_installed_listed_served_replaced_and_removed() {
        let api = test_api().await;
        let (status, location, theme) = upload(&api, package(MANIFEST)).await;
        assert_eq!(status, 201, "{theme}");
        assert_eq!(
            location.as_deref(),
            Some("/api/v1/themes/night-blue/files/theme.css?v=1")
        );
        assert_eq!(theme["id"], "night-blue");
        assert_eq!(theme["packageVersion"], "1.2.0");
        assert_eq!(theme["colorScheme"], "dark");
        assert_eq!(theme["startColor"], "#101018");
        assert_eq!(
            theme["previewUrl"],
            "/api/v1/themes/night-blue/files/assets/preview.png"
        );
        assert_eq!(theme["settings"][0]["type"], "boolean");
        assert_eq!(theme["settings"][0]["key"], "dim");
        assert_eq!(theme["version"], 1);

        let css = file(&api, "/api/v1/themes/night-blue/files/theme.css").await;
        assert_eq!(css.status, 200);
        assert_eq!(css.kind.as_deref(), Some("text/css; charset=utf-8"));
        assert_eq!(css.cache.as_deref(), Some("no-cache"));
        let csp = css.csp.unwrap();
        assert!(csp.starts_with("sandbox; default-src 'none'"), "{csp}");
        assert!(!csp.contains("frame-ancestors"), "{csp}");
        assert_eq!(css.body, b":root { --accent: #6c63ff }");
        let png = file(&api, "/api/v1/themes/night-blue/files/assets/preview.png").await;
        assert_eq!(
            (png.status, png.kind.as_deref(), png.body.as_slice()),
            (200, Some("image/png"), b"PNG".as_slice())
        );

        let newer = MANIFEST.replace("1.2.0", "1.3.0");
        let (status, _, again) = upload(&api, package(&newer)).await;
        assert_eq!(status, 200);
        assert_eq!(
            (&again["packageVersion"], &again["version"]),
            (&json!("1.3.0"), &json!(2))
        );
        let (_, list) = call(&api, "GET", "/api/v1/themes", None).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 1);

        let stale = "/api/v1/themes/night-blue?version=1";
        let (status, problem) = call(&api, "DELETE", stale, None).await;
        assert_eq!(
            (status, &problem["code"]),
            (409, &json!("version-conflict"))
        );
        let current = "/api/v1/themes/night-blue?version=2";
        let (status, _) = call(&api, "DELETE", current, None).await;
        assert_eq!(status, 204);
        let (status, problem) = call(&api, "DELETE", current, None).await;
        assert_eq!((status, &problem["code"]), (404, &json!("theme-not-found")));
        let gone = file(&api, "/api/v1/themes/night-blue/files/theme.css").await;
        assert_eq!(gone.status, 404);
    }

    #[tokio::test]
    async fn invalid_packages_are_refused_with_their_reason() {
        let api = test_api().await;
        let (status, _, problem) = upload(&api, b"manifest.json={}".to_vec()).await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("theme-manifest-invalid"))
        );
        let fields: Vec<&str> = problem["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|error| error["field"].as_str().unwrap())
            .collect();
        assert_eq!(
            fields,
            [
                "id",
                "name",
                "version",
                "author",
                "sdkVersion",
                "colorScheme"
            ]
        );

        let good = String::from_utf8(package(MANIFEST)).unwrap();
        let escaping = format!("{good}\n--\n../evil.css=x");
        let (status, _, problem) = upload(&api, escaping.into_bytes()).await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("theme-package-invalid"))
        );
        let remote = good.replace(":root { --accent: #6c63ff }", "@import 'x.css';");
        let (status, _, _) = upload(&api, remote.into_bytes()).await;
        assert_eq!(status, 422);

        let reserved = package(&MANIFEST.replace("night-blue", "dark"));
        let (status, _, problem) = upload(&api, reserved).await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("theme-id-reserved"))
        );

        let limit = usize::try_from(nullradix_domain::MAX_PACKAGE_BYTES).unwrap();
        let (status, _, problem) = upload(&api, vec![b'a'; limit + 1]).await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("theme-package-invalid"))
        );
        let (_, list) = call(&api, "GET", "/api/v1/themes", None).await;
        assert_eq!(list["items"], json!([]));
    }

    #[tokio::test]
    async fn only_known_themes_can_be_chosen_and_removing_resets_the_choice() {
        let api = test_api().await;
        let choose = |theme: &str| Some(json!({ "theme": theme }));
        let (status, problem) =
            call(&api, "PUT", "/api/v1/settings/theme", choose("night-blue")).await;
        assert_eq!((status, &problem["code"]), (404, &json!("theme-not-found")));
        let (status, problem) =
            call(&api, "PUT", "/api/v1/settings/theme", choose("No Good")).await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("theme-id-invalid"))
        );
        let other = Some(json!({ "other": 1 }));
        let (status, _) = call(&api, "PUT", "/api/v1/settings/theme", other).await;
        assert_eq!(status, 422);

        upload(&api, package(MANIFEST)).await;
        let (status, _) = call(&api, "PUT", "/api/v1/settings/theme", choose("night-blue")).await;
        assert_eq!(status, 200);
        let (_, list) = call(&api, "GET", "/api/v1/themes", None).await;
        assert_eq!(list["active"], "night-blue");
        call(&api, "DELETE", "/api/v1/themes/night-blue?version=1", None).await;
        let (_, list) = call(&api, "GET", "/api/v1/themes", None).await;
        assert_eq!(list["active"], "nullradix");
        let (status, _) = call(&api, "DELETE", "/api/v1/themes/night-blue", None).await;
        assert_eq!(status, 422);
    }

    fn with_mode() -> Vec<u8> {
        let mode = r#""dim": { "type": "boolean", "label": "Dim", "default": true },
            "mode": { "type": "select", "label": "Mode", "default": "a",
                      "options": [{ "value": "a", "label": "A" }, { "value": "b", "label": "B" }] }"#;
        let manifest = MANIFEST.replace(
            r#""dim": { "type": "boolean", "label": "Dim", "default": true }"#,
            mode,
        );
        package(&manifest)
    }

    const SETTINGS: &str = "/api/v1/themes/night-blue/settings";

    #[tokio::test]
    async fn settings_are_read_saved_with_a_version_and_reset() {
        let api = test_api().await;
        upload(&api, with_mode()).await;
        let (status, empty) = call(&api, "GET", SETTINGS, None).await;
        assert_eq!(
            (status, &empty),
            (200, &json!({ "values": {}, "version": 0 }))
        );

        let body = json!({ "values": { "dim": false, "mode": "b" }, "version": 0 });
        let (status, saved) = call(&api, "PUT", SETTINGS, Some(body)).await;
        let expected = json!({ "values": { "dim": false, "mode": "b" }, "version": 1 });
        assert_eq!((status, &saved), (200, &expected));
        let (_, read) = call(&api, "GET", SETTINGS, None).await;
        assert_eq!(read, expected);

        let body = json!({ "values": { "mode": "a" }, "version": 1 });
        let (_, saved) = call(&api, "PUT", SETTINGS, Some(body)).await;
        assert_eq!(saved, json!({ "values": { "mode": "a" }, "version": 2 }));

        let (status, _) = call(&api, "DELETE", &format!("{SETTINGS}?version=2"), None).await;
        assert_eq!(status, 204);
        let (_, read) = call(&api, "GET", SETTINGS, None).await;
        assert_eq!(read, empty);
        let (status, _) = call(&api, "DELETE", &format!("{SETTINGS}?version=0"), None).await;
        assert_eq!(status, 204);
    }

    #[tokio::test]
    async fn stale_versions_are_conflicts_for_saving_and_resetting() {
        let api = test_api().await;
        upload(&api, with_mode()).await;
        let body = json!({ "values": { "dim": true }, "version": 0 });
        call(&api, "PUT", SETTINGS, Some(body.clone())).await;
        for body in [body, json!({ "values": { "dim": true }, "version": 7 })] {
            let (status, problem) = call(&api, "PUT", SETTINGS, Some(body)).await;
            assert_eq!(
                (status, &problem["code"]),
                (409, &json!("version-conflict"))
            );
        }
        for query in ["version=0", "version=3"] {
            let (status, problem) =
                call(&api, "DELETE", &format!("{SETTINGS}?{query}"), None).await;
            assert_eq!(
                (status, &problem["code"]),
                (409, &json!("version-conflict"))
            );
        }
        let (status, problem) = call(&api, "DELETE", &format!("{SETTINGS}?version=-1"), None).await;
        assert_eq!((status, &problem["code"]), (422, &json!("version-invalid")));
        let (status, _) = call(&api, "DELETE", SETTINGS, None).await;
        assert_eq!(status, 422);
        let (_, read) = call(&api, "GET", SETTINGS, None).await;
        assert_eq!(read["version"], 1);
    }

    #[tokio::test]
    async fn refused_values_name_the_field_that_caused_them() {
        let api = test_api().await;
        upload(&api, with_mode()).await;
        let values = json!({ "dim": "yes", "mode": "z", "other": true });
        let body = json!({ "values": values, "version": 0 });
        let (status, problem) = call(&api, "PUT", SETTINGS, Some(body)).await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("theme-settings-invalid"))
        );
        let fields: Vec<&str> = problem["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|error| error["field"].as_str().unwrap())
            .collect();
        assert_eq!(fields, ["dim", "mode", "other"]);
        for body in [
            json!({ "values": { "dim": 1 }, "version": 0 }),
            json!({ "values": { "dim": null }, "version": 0 }),
            json!({ "values": [], "version": 0 }),
            json!({ "values": {} }),
            json!({ "values": {}, "version": 0, "extra": 1 }),
        ] {
            let (status, _) = call(&api, "PUT", SETTINGS, Some(body)).await;
            assert_eq!(status, 422);
        }
        let many: serde_json::Map<String, Value> = (0..51)
            .map(|index| (format!("k{index}"), json!(true)))
            .collect();
        let body = json!({ "values": many, "version": 0 });
        let (status, problem) = call(&api, "PUT", SETTINGS, Some(body)).await;
        assert_eq!(
            (status, &problem["errors"][0]["field"]),
            (422, &json!("values"))
        );
        let (_, read) = call(&api, "GET", SETTINGS, None).await;
        assert_eq!(read["version"], 0);
    }

    #[tokio::test]
    async fn settings_of_themes_that_are_not_installed_are_not_found() {
        let api = test_api().await;
        let body = json!({ "values": {}, "version": 0 });
        let (status, problem) = call(&api, "PUT", SETTINGS, Some(body)).await;
        assert_eq!((status, &problem["code"]), (404, &json!("theme-not-found")));
        let (status, _) = call(&api, "GET", SETTINGS, None).await;
        assert_eq!(status, 404);
        let (status, _) = call(&api, "DELETE", &format!("{SETTINGS}?version=0"), None).await;
        assert_eq!(status, 404);
        let (status, problem) = call(&api, "GET", "/api/v1/themes/No%20Good/settings", None).await;
        assert_eq!(
            (status, &problem["code"]),
            (422, &json!("theme-id-invalid"))
        );
    }

    #[tokio::test]
    async fn uninstalling_a_theme_takes_its_settings_along() {
        let api = test_api().await;
        upload(&api, with_mode()).await;
        let body = json!({ "values": { "dim": false }, "version": 0 });
        call(&api, "PUT", SETTINGS, Some(body)).await;
        call(&api, "DELETE", "/api/v1/themes/night-blue?version=1", None).await;
        upload(&api, with_mode()).await;
        let (_, read) = call(&api, "GET", SETTINGS, None).await;
        assert_eq!(read, json!({ "values": {}, "version": 0 }));
    }

    #[tokio::test]
    async fn theme_files_outside_the_package_are_never_served() {
        let api = test_api().await;
        upload(&api, package(MANIFEST)).await;
        for path in [
            "/api/v1/themes/night-blue/files/missing.css",
            "/api/v1/themes/night-blue/files/..%2Fmanifest.json",
            "/api/v1/themes/night-blue/files/%2e%2e%2fnight-blue%2ftheme.css",
            "/api/v1/themes/night-blue/files/assets%2F..%2Ftheme.css",
            "/api/v1/themes/night-blue/files/%5Cmanifest.json",
            "/api/v1/themes/night-blue/files/C%3A%2Fwindows",
            "/api/v1/themes/night-blue/files/assets/",
            "/api/v1/themes/other/files/theme.css",
            "/api/v1/themes/..%2Fnight-blue/files/theme.css",
            "/api/v1/themes/Night%20Blue/files/theme.css",
        ] {
            let reply = file(&api, path).await;
            assert_eq!(reply.status, 404, "{path}");
            assert_eq!(
                reply.kind.as_deref(),
                Some("application/problem+json"),
                "{path}"
            );
            let problem: Value = serde_json::from_slice(&reply.body).unwrap();
            assert_eq!(problem["code"], "theme-file-not-found", "{path}");
        }
        let manifest = file(&api, "/api/v1/themes/night-blue/files/manifest.json").await;
        assert_eq!(manifest.status, 200);
    }

    #[tokio::test]
    async fn themes_need_a_login() {
        let api = test_api().await;
        for request in [
            local(Request::get("/api/v1/themes")),
            local(Request::get("/api/v1/themes/night-blue/files/theme.css")),
            local(Request::post("/api/v1/themes")),
            local(Request::put("/api/v1/settings/theme")),
        ] {
            let (status, _, _) = send(&api.router, request.body(Body::empty()).unwrap()).await;
            assert_eq!(status, 401);
        }
    }

    fn with_code(manifest: &str) -> Vec<u8> {
        let mut bytes = package(manifest);
        bytes.extend_from_slice(b"\n--\nmain.js=run()");
        bytes
    }

    #[tokio::test]
    async fn a_package_with_code_needs_the_confirmed_flag() {
        let api = test_api().await;
        let domains = MANIFEST.replace(r#""network": []"#, r#""network": ["Fonts.Example.com"]"#);
        let (status, _, problem) = upload(&api, with_code(&domains)).await;
        assert_eq!(status, 409, "{problem}");
        assert_eq!(problem["code"], "theme-confirmation-required");
        assert_eq!(
            problem["confirmation"],
            json!({
                "hasCode": true, "domains": ["fonts.example.com"], "claudeTheme": false,
                "update": false, "codeAdded": true, "domainsAdded": ["fonts.example.com"]
            })
        );
        let (_, list) = call(&api, "GET", "/api/v1/themes", None).await;
        assert!(list["items"].as_array().unwrap().is_empty());
        let confirmed = "/api/v1/themes?confirmed=true";
        let (status, _, theme) = upload_to(&api, confirmed, with_code(&domains)).await;
        assert_eq!((status, &theme["id"]), (201, &json!("night-blue")));

        let newer = domains.replace("1.2.0", "1.3.0");
        let (status, _, again) = upload(&api, with_code(&newer)).await;
        assert_eq!((status, &again["version"]), (200, &json!(2)));
        let more = newer.replace("Fonts.Example.com", "cdn.example.com");
        let (status, _, problem) = upload(&api, with_code(&more)).await;
        assert_eq!(status, 409, "{problem}");
        assert_eq!(problem["confirmation"]["update"], true);
        assert_eq!(problem["confirmation"]["codeAdded"], false);
        assert_eq!(
            problem["confirmation"]["domainsAdded"],
            json!(["cdn.example.com"])
        );
    }
}
