//! Theme catalogs: sources, the themes they offer and installing one.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use nullradix_app::AppError;
use nullradix_app::theme_catalog::{
    Catalog, CatalogTheme, SkippedEntry, SourceList, SourceProblem, SourceStatus,
};
use nullradix_domain::{Availability, CatalogSource, MAX_VERSION_CHARS, THEME_ID_MAX};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::themes::ThemeDto;
use crate::{ApiError, ApiJson, ApiPath, ApiQuery, ApiState, Problem};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSourceDto {
    /// Stable id derived from the address.
    pub id: String,
    pub url: String,
    pub added_at: i64,
}

impl From<&CatalogSource> for CatalogSourceDto {
    fn from(source: &CatalogSource) -> Self {
        Self {
            id: source.id().to_string(),
            url: source.url().to_string(),
            added_at: source.added_at().as_millis(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSourceListDto {
    /// In the order they were added.
    pub items: Vec<CatalogSourceDto>,
    /// Version of the list for removing; 0 while no source was ever added.
    pub version: i64,
}

impl From<&SourceList> for CatalogSourceListDto {
    fn from(list: &SourceList) -> Self {
        Self {
            items: list.sources.iter().map(CatalogSourceDto::from).collect(),
            version: list.version.map_or(0, nullradix_domain::Version::value),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddCatalogSource {
    /// An https address of an index file, a JSON list of themes.
    pub url: String,
}

/// What a catalog theme means for this installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CatalogStateDto {
    Available,
    Installed,
    /// A newer version than the installed one is offered.
    Update,
    /// Installed from another source or by upload; installing needs `replace`.
    OtherSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogThemeDto {
    pub source_id: String,
    pub id: String,
    pub name: String,
    pub version: String,
    /// Range of SDK versions the theme runs with.
    pub sdk_version: String,
    /// Whether the theme fits this version; incompatible themes cannot be installed.
    pub compatible: bool,
    pub description: Option<String>,
    pub state: CatalogStateDto,
    /// The installed version when the theme is installed.
    pub installed_version: Option<String>,
}

/// A version string as long as an entry may state it.
fn capped(text: &str) -> String {
    text.chars().take(MAX_VERSION_CHARS).collect()
}

impl From<&CatalogTheme> for CatalogThemeDto {
    fn from(theme: &CatalogTheme) -> Self {
        let entry = &theme.entry;
        let (state, installed_version) = match &theme.availability {
            Availability::NotInstalled => (CatalogStateDto::Available, None),
            Availability::Installed(version) => (CatalogStateDto::Installed, Some(version.clone())),
            Availability::Update(version) => (CatalogStateDto::Update, Some(version.clone())),
            Availability::OtherSource(version) => {
                (CatalogStateDto::OtherSource, Some(version.clone()))
            }
        };
        Self {
            source_id: theme.source.to_string(),
            id: entry.id.to_string(),
            name: entry.name.clone(),
            version: capped(&entry.version),
            sdk_version: capped(&entry.sdk_version),
            compatible: entry.compatible,
            description: entry.description.clone(),
            state,
            installed_version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogProblemDto {
    /// Stable code such as `catalog-timeout` or `catalog-index-invalid`.
    pub code: String,
    pub message: String,
}

impl From<&SourceProblem> for CatalogProblemDto {
    fn from(problem: &SourceProblem) -> Self {
        Self {
            code: problem.code.to_owned(),
            message: problem.message.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkippedEntryDto {
    /// Position of the entry in the index.
    pub index: usize,
    pub id: Option<String>,
    pub reason: String,
}

impl From<&SkippedEntry> for SkippedEntryDto {
    fn from(skipped: &SkippedEntry) -> Self {
        Self {
            index: skipped.index,
            id: skipped
                .id
                .as_deref()
                .map(|id| id.chars().take(THEME_ID_MAX).collect()),
            reason: skipped.reason.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSourceStatusDto {
    pub source: CatalogSourceDto,
    /// Why the source could not be read; its themes are missing then.
    pub problem: Option<CatalogProblemDto>,
    /// Valid entries of the source.
    pub offered: usize,
    pub skipped: Vec<SkippedEntryDto>,
}

impl From<&SourceStatus> for CatalogSourceStatusDto {
    fn from(status: &SourceStatus) -> Self {
        Self {
            source: CatalogSourceDto::from(&status.source),
            problem: status.problem.as_ref().map(CatalogProblemDto::from),
            offered: status.offered,
            skipped: status.skipped.iter().map(SkippedEntryDto::from).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogDto {
    /// Sorted by name, then id, then source.
    pub items: Vec<CatalogThemeDto>,
    pub sources: Vec<CatalogSourceStatusDto>,
}

impl From<&Catalog> for CatalogDto {
    fn from(catalog: &Catalog) -> Self {
        Self {
            items: catalog.themes.iter().map(CatalogThemeDto::from).collect(),
            sources: catalog
                .sources
                .iter()
                .map(CatalogSourceStatusDto::from)
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct SourcePath {
    /// Source id.
    id: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct InstallPath {
    /// Source id.
    id: String,
    /// Theme id as the source offers it.
    theme: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct InstallQuery {
    /// Replace a theme of the same id that was installed from another source or by upload.
    #[serde(default)]
    replace: bool,
    /// Checksum of the package the person confirmed, from the `confirmation` of the refusal.
    #[serde(default, rename = "confirmedSha256")]
    confirmed_sha256: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct RemoveSourceQuery {
    /// Version of the source list the removal is based on.
    version: i64,
}

#[utoipa::path(
    get,
    path = "/theme-catalog/sources",
    tag = "themes",
    responses((status = 200, body = CatalogSourceListDto), (status = 401, body = Problem))
)]
pub(crate) async fn list_catalog_sources(
    State(state): State<ApiState>,
) -> Result<Json<CatalogSourceListDto>, ApiError> {
    Ok(Json(CatalogSourceListDto::from(
        &state.catalog.sources().await?,
    )))
}

#[utoipa::path(
    post,
    path = "/theme-catalog/sources",
    tag = "themes",
    request_body = AddCatalogSource,
    responses(
        (status = 201, description = "Added", body = CatalogSourceDto,
            headers(("Location" = String, description = "URL of the source list"))),
        (status = 401, body = Problem),
        (status = 409, description = "`catalog-source-exists`", body = Problem),
        (status = 422, description = "`catalog-source-invalid` (not https, private host, credentials, too long) or `catalog-source-limit`", body = Problem),
    )
)]
pub(crate) async fn add_catalog_source(
    State(state): State<ApiState>,
    ApiJson(input): ApiJson<AddCatalogSource>,
) -> Result<(StatusCode, HeaderMap, Json<CatalogSourceDto>), ApiError> {
    let source = state.catalog.add_source(&input.url).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_static("/api/v1/theme-catalog/sources"),
    );
    Ok((
        StatusCode::CREATED,
        headers,
        Json(CatalogSourceDto::from(&source)),
    ))
}

#[utoipa::path(
    delete,
    path = "/theme-catalog/sources/{id}",
    tag = "themes",
    params(SourcePath, RemoveSourceQuery),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, body = Problem),
        (status = 404, description = "`catalog-source-not-found`", body = Problem),
        (status = 409, description = "Stale version of the list", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn remove_catalog_source(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<SourcePath>,
    ApiQuery(query): ApiQuery<RemoveSourceQuery>,
) -> Result<StatusCode, ApiError> {
    state
        .catalog
        .remove_source(&path.id, AppError::client_version(query.version)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/theme-catalog/themes",
    tag = "themes",
    responses(
        (status = 200, description = "The themes of every source; a source that fails is reported in `sources`", body = CatalogDto),
        (status = 401, body = Problem),
    )
)]
pub(crate) async fn browse_catalog(
    State(state): State<ApiState>,
) -> Result<Json<CatalogDto>, ApiError> {
    Ok(Json(CatalogDto::from(&state.catalog.browse().await?)))
}

#[utoipa::path(
    post,
    path = "/theme-catalog/sources/{id}/themes/{theme}/install",
    tag = "themes",
    params(InstallPath, InstallQuery),
    responses(
        (status = 201, description = "Installed", body = ThemeDto,
            headers(("Location" = String, description = "URL of the stylesheet"))),
        (status = 200, description = "Replaced the installed theme with the same id", body = ThemeDto),
        (status = 401, body = Problem),
        (status = 404, description = "`catalog-source-not-found` or `catalog-theme-not-found`", body = Problem),
        (status = 409, description = "`catalog-theme-incompatible`, `catalog-theme-installed-elsewhere` (retry with `replace=true`) or `theme-confirmation-required` (retry with `confirmedSha256` from the problem)", body = Problem),
        (status = 422, description = "`catalog-checksum-mismatch`, `theme-package-mismatch`, `catalog-too-large`, `catalog-address-refused`, or the package is invalid as an uploaded one", body = Problem),
        (status = 503, description = "The source cannot be reached: `catalog-timeout`, `catalog-unreachable`, `catalog-http-error`", body = Problem),
    )
)]
pub(crate) async fn install_catalog_theme(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<InstallPath>,
    ApiQuery(query): ApiQuery<InstallQuery>,
) -> Result<(StatusCode, HeaderMap, Json<ThemeDto>), ApiError> {
    let installed = state
        .catalog
        .install(
            &path.id,
            &path.theme,
            query.replace,
            query.confirmed_sha256.as_deref(),
        )
        .await?;
    crate::claude_theme::resync(&state).await;
    let dto = ThemeDto::new(&installed.theme, installed.code);
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&dto.style_url) {
        headers.insert(header::LOCATION, value);
    }
    let status = if installed.theme.record().version.value() == 1 {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, headers, Json(dto)))
}

#[cfg(test)]
pub(crate) mod fake {
    use std::collections::HashMap;

    use async_trait::async_trait;
    use nullradix_app::theme_catalog::{CatalogFetcher, FetchError};
    use nullradix_domain::CatalogUrl;
    use parking_lot::Mutex;

    /// Answers by address; a missing answer is unreachable.
    #[derive(Default)]
    pub(crate) struct Fetcher {
        answers: Mutex<HashMap<String, Result<Vec<u8>, FetchError>>>,
        pub(crate) asked: Mutex<Vec<String>>,
    }

    impl Fetcher {
        pub(crate) fn answer(&self, url: &str, result: Result<Vec<u8>, FetchError>) {
            self.answers.lock().insert(url.to_owned(), result);
        }
    }

    #[async_trait]
    impl CatalogFetcher for Fetcher {
        async fn fetch(&self, url: &CatalogUrl, _max_bytes: u64) -> Result<Vec<u8>, FetchError> {
            self.asked.lock().push(url.to_string());
            self.answers
                .lock()
                .get(url.as_str())
                .cloned()
                .unwrap_or(Err(FetchError::Unreachable))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use axum::body::Body;
    use axum::http::{Request, header};
    use nullradix_app::theme_catalog::FetchError;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    use crate::tests::{TestApi, bearer, local, send, test_api};

    const MANIFEST: &str = r##"{
        "id": "night-blue", "name": "Night Blue", "version": "1.2.0", "author": "Ada",
        "sdkVersion": "^1.3", "colorScheme": "dark", "startColor": "#101018",
        "slots": [], "permissions": { "network": [] }
    }"##;
    const SOURCE: &str = "https://themes.example.com/index.json";
    const PACKAGE: &str = "https://themes.example.com/night-blue.nullradix";

    fn package() -> Vec<u8> {
        format!("manifest.json={MANIFEST}\n--\ntheme.css=:root {{ --accent: #6c63ff }}")
            .into_bytes()
    }

    fn index(bytes: &[u8]) -> Vec<u8> {
        let mut sha = String::new();
        for byte in Sha256::digest(bytes) {
            let _ = write!(sha, "{byte:02x}");
        }
        json!([
            { "id": "night-blue", "name": "Night Blue", "version": "1.2.0", "sdkVersion": "^1.3",
              "downloadUrl": PACKAGE, "sha256": sha, "description": "Dark" },
            { "id": "broken" }
        ])
        .to_string()
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

    async fn add(api: &TestApi, url: &str) -> (u16, Value) {
        call(
            api,
            "POST",
            "/api/v1/theme-catalog/sources",
            Some(json!({ "url": url })),
        )
        .await
    }

    #[tokio::test]
    async fn sources_are_added_listed_and_removed() {
        let api = test_api().await;
        let (status, list) = call(&api, "GET", "/api/v1/theme-catalog/sources", None).await;
        assert_eq!(
            (status, &list),
            (200, &json!({ "items": [], "version": 0 }))
        );

        let (status, added) = add(&api, SOURCE).await;
        assert_eq!(status, 201);
        assert_eq!(added["url"], SOURCE);
        let id = added["id"].as_str().unwrap().to_owned();
        let (_, list) = call(&api, "GET", "/api/v1/theme-catalog/sources", None).await;
        assert_eq!(
            (list["items"][0]["id"].as_str(), &list["version"]),
            (Some(id.as_str()), &json!(1))
        );

        let path = format!("/api/v1/theme-catalog/sources/{id}?version=9");
        let (status, problem) = call(&api, "DELETE", &path, None).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (409, Some("version-conflict"))
        );
        let path = format!("/api/v1/theme-catalog/sources/{id}?version=1");
        assert_eq!(call(&api, "DELETE", &path, None).await.0, 204);
        let (status, problem) = call(&api, "DELETE", &path, None).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (404, Some("catalog-source-not-found"))
        );
    }

    #[tokio::test]
    async fn refused_addresses_answer_422_and_a_repeat_409() {
        let api = test_api().await;
        for bad in [
            "http://themes.example.com/i.json",
            "https://127.0.0.1/i.json",
            "https://localhost/i.json",
            "https://169.254.169.254/latest/meta-data",
            "https://user:secret@themes.example.com/i.json",
            "file:///etc/passwd",
            "",
        ] {
            let (status, problem) = add(&api, bad).await;
            assert_eq!(
                (status, problem["code"].as_str()),
                (422, Some("catalog-source-invalid")),
                "{bad}"
            );
        }
        assert_eq!(add(&api, SOURCE).await.0, 201);
        let (status, problem) = add(&api, "HTTPS://Themes.Example.com/index.json").await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (409, Some("catalog-source-exists"))
        );
        let request = bearer(Request::post("/api/v1/theme-catalog/sources"), &api.token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"url":"https://a.example.com/","extra":1}"#))
            .unwrap();
        assert_eq!(send(&api.router, request).await.0, 422);
    }

    #[tokio::test]
    async fn the_catalog_lists_themes_and_reports_broken_sources() {
        let api = test_api().await;
        let bytes = package();
        add(&api, SOURCE).await;
        add(&api, "https://down.example.com/i.json").await;
        api.fetcher.answer(SOURCE, Ok(index(&bytes)));
        api.fetcher
            .answer("https://down.example.com/i.json", Err(FetchError::Timeout));
        let (status, catalog) = call(&api, "GET", "/api/v1/theme-catalog/themes", None).await;
        assert_eq!(status, 200);
        let item = &catalog["items"][0];
        assert_eq!(
            (
                item["id"].as_str(),
                item["state"].as_str(),
                item["compatible"].as_bool()
            ),
            (Some("night-blue"), Some("available"), Some(true))
        );
        assert_eq!(catalog["items"].as_array().unwrap().len(), 1);
        assert_eq!(catalog["sources"][0]["offered"], 1);
        assert_eq!(catalog["sources"][0]["skipped"][0]["id"], "broken");
        assert_eq!(catalog["sources"][1]["problem"]["code"], "catalog-timeout");
    }

    #[tokio::test]
    async fn installing_from_the_catalog_installs_the_theme_and_marks_it_installed() {
        let api = test_api().await;
        let bytes = package();
        let (_, added) = add(&api, SOURCE).await;
        let id = added["id"].as_str().unwrap();
        api.fetcher.answer(SOURCE, Ok(index(&bytes)));
        api.fetcher.answer(PACKAGE, Ok(bytes));

        let path = format!("/api/v1/theme-catalog/sources/{id}/themes/night-blue/install");
        let (status, theme) = call(&api, "POST", &path, None).await;
        assert_eq!((status, theme["id"].as_str()), (201, Some("night-blue")));
        let (_, list) = call(&api, "GET", "/api/v1/themes", None).await;
        assert_eq!(list["items"][0]["packageVersion"], "1.2.0");
        let (_, catalog) = call(&api, "GET", "/api/v1/theme-catalog/themes", None).await;
        assert_eq!(catalog["items"][0]["state"], "installed");
        let (status, _) = call(&api, "POST", &path, None).await;
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn installing_from_the_catalog_brings_the_claude_folders_in_line() {
        let api = test_api().await;
        let bytes = package();
        let (_, added) = add(&api, SOURCE).await;
        let id = added["id"].as_str().unwrap();
        api.fetcher.answer(SOURCE, Ok(index(&bytes)));
        api.fetcher.answer(PACKAGE, Ok(bytes));
        let before = api.slots.calls();
        let path = format!("/api/v1/theme-catalog/sources/{id}/themes/night-blue/install");
        let (status, _) = call(&api, "POST", &path, None).await;
        assert_eq!(status, 201);
        assert!(api.slots.calls() > before);
    }

    #[tokio::test]
    async fn a_theme_from_elsewhere_is_replaced_only_on_request() {
        let api = test_api().await;
        let bytes = package();
        let (_, added) = add(&api, SOURCE).await;
        let id = added["id"].as_str().unwrap();
        api.fetcher.answer(SOURCE, Ok(index(&bytes)));
        api.fetcher.answer(PACKAGE, Ok(bytes.clone()));
        let upload = bearer(Request::post("/api/v1/themes"), &api.token)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(bytes))
            .unwrap();
        assert!(send(&api.router, upload).await.0.is_success());
        let (_, catalog) = call(&api, "GET", "/api/v1/theme-catalog/themes", None).await;
        assert_eq!(catalog["items"][0]["state"], "other-source");

        let path = format!("/api/v1/theme-catalog/sources/{id}/themes/night-blue/install");
        let (status, problem) = call(&api, "POST", &path, None).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (409, Some("catalog-theme-installed-elsewhere"))
        );
        let (status, _) = call(&api, "POST", &format!("{path}?replace=true"), None).await;
        assert_eq!(status, 200);
        let (_, catalog) = call(&api, "GET", "/api/v1/theme-catalog/themes", None).await;
        assert_eq!(catalog["items"][0]["state"], "installed");
    }

    #[tokio::test]
    async fn installing_refuses_tampered_unknown_and_unreachable_downloads() {
        let api = test_api().await;
        let bytes = package();
        let (_, added) = add(&api, SOURCE).await;
        let id = added["id"].as_str().unwrap();
        api.fetcher.answer(SOURCE, Ok(index(&bytes)));
        api.fetcher.answer(PACKAGE, Ok(b"tampered".to_vec()));
        let path = format!("/api/v1/theme-catalog/sources/{id}/themes/night-blue/install");
        let (status, problem) = call(&api, "POST", &path, None).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (422, Some("catalog-checksum-mismatch"))
        );

        let path_unknown = format!("/api/v1/theme-catalog/sources/{id}/themes/ghost-theme/install");
        let (status, problem) = call(&api, "POST", &path_unknown, None).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (404, Some("catalog-theme-not-found"))
        );
        let (status, problem) = call(
            &api,
            "POST",
            "/api/v1/theme-catalog/sources/000000000000/themes/night-blue/install",
            None,
        )
        .await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (404, Some("catalog-source-not-found"))
        );

        api.fetcher.answer(PACKAGE, Err(FetchError::Unreachable));
        let (status, problem) = call(&api, "POST", &path, None).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (503, Some("catalog-unreachable"))
        );
        let (_, list) = call(&api, "GET", "/api/v1/themes", None).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn the_routes_need_a_login() {
        let api = test_api().await;
        for (method, path) in [
            ("GET", "/api/v1/theme-catalog/sources"),
            ("POST", "/api/v1/theme-catalog/sources"),
            ("GET", "/api/v1/theme-catalog/themes"),
            (
                "DELETE",
                "/api/v1/theme-catalog/sources/abcdef123456?version=1",
            ),
            (
                "POST",
                "/api/v1/theme-catalog/sources/abcdef123456/themes/x-y/install",
            ),
        ] {
            let request = local(Request::builder().method(method).uri(path))
                .body(Body::empty())
                .unwrap();
            assert_eq!(send(&api.router, request).await.0, 401, "{method} {path}");
        }
        assert!(api.fetcher.asked.lock().is_empty());
    }

    #[tokio::test]
    async fn a_catalog_theme_with_code_is_installed_only_when_confirmed() {
        let api = test_api().await;
        let mut bytes = package();
        bytes.extend_from_slice(b"\n--\nmain.js=run()");
        let (_, added) = add(&api, SOURCE).await;
        let id = added["id"].as_str().unwrap();
        api.fetcher.answer(SOURCE, Ok(index(&bytes)));
        api.fetcher.answer(PACKAGE, Ok(bytes));
        let path = format!("/api/v1/theme-catalog/sources/{id}/themes/night-blue/install");
        let (status, problem) = call(&api, "POST", &path, None).await;
        assert_eq!(status, 409, "{problem}");
        assert_eq!(problem["code"], "theme-confirmation-required");
        assert_eq!(problem["confirmation"]["hasCode"], true);
        let (_, list) = call(&api, "GET", "/api/v1/themes", None).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 0);
        let sha = problem["confirmation"]["sha256"]
            .as_str()
            .unwrap()
            .to_owned();
        let wrong = format!("{path}?confirmedSha256={}", "0".repeat(64));
        let (status, problem) = call(&api, "POST", &wrong, None).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (409, Some("theme-confirmation-required"))
        );
        let right = format!("{path}?confirmedSha256={sha}");
        let (status, theme) = call(&api, "POST", &right, None).await;
        assert_eq!((status, theme["id"].as_str()), (201, Some("night-blue")));
    }
}
