//! Theme catalogs: sources the person added, the themes they offer and installing one.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use nullradix_domain::{
    Availability, CatalogEntry, CatalogError, CatalogSource, CatalogUrl, EntryDraft,
    InstalledTheme, MAX_INDEX_BYTES, MAX_INDEX_ENTRIES, MAX_PACKAGE_BYTES, MAX_SOURCES, Reach,
    SourceId, ThemeId, Timestamp, Version,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::themes::ThemeService;
use crate::{AppError, Change, ChangeSignal, Clock};

/// Entity id of the catalog sources in the change feed, which only takes UUIDs.
pub const SOURCES_FEED_ID: Uuid = Uuid::from_u128(0x6e75_6c6c_7261_6469_7800_6361_7461_6c67);
/// Sources asked at the same time.
const PARALLEL_SOURCES: usize = 10;

/// Why a download failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    /// The address, a redirect or the resolved host is outside what is allowed.
    Refused,
    Timeout,
    Unreachable,
    /// The server answered with this status.
    Status(u16),
    TooLarge,
}

/// Downloads a file; adapters enforce https, public hosts, redirects, time and size.
#[async_trait]
pub trait CatalogFetcher: Send + Sync {
    /// The body of the address; more than `max_bytes` fails with `TooLarge`.
    async fn fetch(&self, url: &CatalogUrl, max_bytes: u64) -> Result<Vec<u8>, FetchError>;
}

/// Fetcher for hosts that cannot reach the network: every download is refused.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoNetwork;

#[async_trait]
impl CatalogFetcher for NoNetwork {
    async fn fetch(&self, _url: &CatalogUrl, _max_bytes: u64) -> Result<Vec<u8>, FetchError> {
        Err(FetchError::Unreachable)
    }
}

/// The stored sources and the version of their row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSources {
    pub sources: Vec<CatalogSource>,
    pub version: Version,
}

/// The sources in the order they were added; `version` is `None` until one was added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceList {
    pub sources: Vec<CatalogSource>,
    pub version: Option<Version>,
}

/// One write; dropping it without commit rolls back.
#[async_trait]
pub trait SourcesWrite: Send {
    async fn sources(&mut self) -> Result<Option<StoredSources>, AppError>;
    /// Replaces the list when the row is at `expected` (`None`: no row yet), else a conflict.
    async fn put(
        &mut self,
        sources: &[CatalogSource],
        expected: Option<Version>,
        at: Timestamp,
    ) -> Result<(Version, Change), AppError>;
    async fn commit(self: Box<Self>) -> Result<(), AppError>;
}

#[async_trait]
pub trait SourcesRepository: Send + Sync {
    async fn begin(&self) -> Result<Box<dyn SourcesWrite>, AppError>;
    async fn sources(&self) -> Result<Option<StoredSources>, AppError>;
}

/// The sources as the JSON that is stored.
#[must_use]
pub fn sources_json(sources: &[CatalogSource]) -> Value {
    Value::Array(
        sources
            .iter()
            .map(|source| {
                let mut object = Map::new();
                object.insert(String::from("url"), Value::from(source.url().as_str()));
                object.insert(
                    String::from("addedAt"),
                    Value::from(source.added_at().as_millis()),
                );
                Value::Object(object)
            })
            .collect(),
    )
}

/// Reads the stored JSON; `None` when it is not what `sources_json` writes.
#[must_use]
pub fn parse_sources(text: &str) -> Option<Vec<CatalogSource>> {
    let Value::Array(items) = serde_json::from_str::<Value>(text).ok()? else {
        return None;
    };
    items
        .iter()
        .map(|item| {
            let url = CatalogUrl::parse(item.get("url")?.as_str()?, Reach::Anywhere).ok()?;
            let added = item.get("addedAt")?.as_i64()?;
            Some(CatalogSource::new(url, Timestamp::from_millis(added)))
        })
        .collect()
}

/// An index entry that was left out and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedEntry {
    /// Position in the index.
    pub index: usize,
    pub id: Option<String>,
    pub reason: String,
}

/// The valid entries of an index and the skipped ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedIndex {
    pub entries: Vec<CatalogEntry>,
    pub skipped: Vec<SkippedEntry>,
}

/// Why a whole index is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct IndexError(String);

fn text_field(fields: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match fields.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("{key} must be text")),
    }
}

fn draft(item: &Value) -> Result<EntryDraft, String> {
    let Value::Object(fields) = item else {
        return Err(String::from("the entry is not an object"));
    };
    Ok(EntryDraft {
        id: text_field(fields, "id")?,
        name: text_field(fields, "name")?,
        version: text_field(fields, "version")?,
        sdk_version: text_field(fields, "sdkVersion")?,
        download_url: text_field(fields, "downloadUrl")?,
        sha256: text_field(fields, "sha256")?,
        description: text_field(fields, "description")?,
    })
}

/// Reads an index; bad entries are skipped with a reason, a bad list is refused.
pub fn parse_index(bytes: &[u8], reach: Reach) -> Result<ParsedIndex, IndexError> {
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err(IndexError(String::from("the index is too large")));
    }
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| IndexError(String::from("the index is not valid JSON")))?;
    let Value::Array(items) = value else {
        return Err(IndexError(String::from("the index must be a JSON list")));
    };
    if items.len() > MAX_INDEX_ENTRIES {
        return Err(IndexError(format!(
            "the index has more than {MAX_INDEX_ENTRIES} entries"
        )));
    }
    let mut parsed = ParsedIndex::default();
    let mut seen = HashSet::new();
    for (index, item) in items.iter().enumerate() {
        let id = item.get("id").and_then(Value::as_str).map(str::to_owned);
        let built = draft(item).and_then(|draft| CatalogEntry::build(&draft, reach));
        match built {
            Ok(entry) if seen.insert(entry.id.clone()) => parsed.entries.push(entry),
            Ok(_) => parsed.skipped.push(SkippedEntry {
                index,
                id,
                reason: String::from("the id appears twice in this index"),
            }),
            Err(reason) => parsed.skipped.push(SkippedEntry { index, id, reason }),
        }
    }
    Ok(parsed)
}

/// A source that could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProblem {
    pub code: &'static str,
    pub message: String,
}

/// What asking one source gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStatus {
    pub source: CatalogSource,
    pub problem: Option<SourceProblem>,
    pub offered: usize,
    pub skipped: Vec<SkippedEntry>,
}

/// A theme a source offers, with what it means for this installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogTheme {
    pub source: SourceId,
    pub entry: CatalogEntry,
    pub availability: Availability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    /// Sorted by name, then id, then source.
    pub themes: Vec<CatalogTheme>,
    pub sources: Vec<SourceStatus>,
}

fn fetch_problem(error: FetchError, what: &str) -> SourceProblem {
    match error {
        FetchError::Refused => SourceProblem {
            code: "catalog-address-refused",
            message: format!("the address of {what} is not allowed"),
        },
        FetchError::Timeout => SourceProblem {
            code: "catalog-timeout",
            message: format!("{what} did not answer in time"),
        },
        FetchError::Unreachable => SourceProblem {
            code: "catalog-unreachable",
            message: format!("{what} cannot be reached"),
        },
        FetchError::Status(status) => SourceProblem {
            code: "catalog-http-error",
            message: format!("{what} answered with HTTP {status}"),
        },
        FetchError::TooLarge => SourceProblem {
            code: "catalog-too-large",
            message: format!("{what} is larger than allowed"),
        },
    }
}

fn fetch_failed(error: FetchError, what: &str) -> AppError {
    let problem = fetch_problem(error, what);
    match error {
        FetchError::TooLarge | FetchError::Refused => {
            AppError::invalid(problem.code, problem.message)
        }
        _ => AppError::unavailable(problem.code, problem.message),
    }
}

fn source_invalid(error: &CatalogError) -> AppError {
    AppError::invalid("catalog-source-invalid", error.to_string())
}

fn source_not_found() -> AppError {
    AppError::not_found("catalog-source-not-found", "no such catalog source")
}

fn sha256_hex(bytes: &[u8]) -> String {
    crate::auth::hex(&Sha256::digest(bytes))
}

pub struct CatalogService {
    repository: Arc<dyn SourcesRepository>,
    fetcher: Arc<dyn CatalogFetcher>,
    themes: Arc<ThemeService>,
    clock: Arc<dyn Clock>,
    signal: ChangeSignal,
    reach: Reach,
}

impl std::fmt::Debug for CatalogService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogService").finish_non_exhaustive()
    }
}

impl CatalogService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn SourcesRepository>,
        fetcher: Arc<dyn CatalogFetcher>,
        themes: Arc<ThemeService>,
        clock: Arc<dyn Clock>,
        signal: ChangeSignal,
        reach: Reach,
    ) -> Self {
        Self {
            repository,
            fetcher,
            themes,
            clock,
            signal,
            reach,
        }
    }

    pub async fn sources(&self) -> Result<SourceList, AppError> {
        Ok(match self.repository.sources().await? {
            Some(stored) => SourceList {
                sources: stored.sources,
                version: Some(stored.version),
            },
            None => SourceList {
                sources: Vec::new(),
                version: None,
            },
        })
    }

    /// Adds the address as a source; the same address twice or too many sources are refused.
    pub async fn add_source(&self, url: &str) -> Result<CatalogSource, AppError> {
        let url = CatalogUrl::parse(url, self.reach).map_err(|error| source_invalid(&error))?;
        let source = CatalogSource::new(url, self.clock.now());
        let mut write = self.repository.begin().await?;
        let stored = write.sources().await?;
        let mut sources = stored
            .as_ref()
            .map_or_else(Vec::new, |stored| stored.sources.clone());
        if sources.iter().any(|known| known.id() == source.id()) {
            return Err(AppError::conflict(
                "catalog-source-exists",
                "this address is already a source",
            )
            .with_existing(source.id().to_string()));
        }
        if sources.len() >= MAX_SOURCES {
            return Err(AppError::invalid(
                "catalog-source-limit",
                format!("at most {MAX_SOURCES} sources are allowed"),
            ));
        }
        sources.push(source.clone());
        let (_, change) = write
            .put(
                &sources,
                stored.map(|stored| stored.version),
                self.clock.now(),
            )
            .await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(source)
    }

    /// Removes the source; `expected` is the version of the list the caller read.
    pub async fn remove_source(&self, id: &str, expected: Version) -> Result<(), AppError> {
        let id = SourceId::parse(id).ok_or_else(source_not_found)?;
        let mut write = self.repository.begin().await?;
        let stored = write.sources().await?.ok_or_else(source_not_found)?;
        if !stored.sources.iter().any(|source| source.id() == &id) {
            return Err(source_not_found());
        }
        if stored.version != expected {
            return Err(AppError::version_conflict("catalog sources", &id));
        }
        let remaining: Vec<CatalogSource> = stored
            .sources
            .into_iter()
            .filter(|source| source.id() != &id)
            .collect();
        let (_, change) = write
            .put(&remaining, Some(expected), self.clock.now())
            .await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(())
    }

    /// Asks every source for its themes; a failing source shows as a problem, not an error.
    pub async fn browse(&self) -> Result<Catalog, AppError> {
        let sources = self.sources().await?.sources;
        let installed: BTreeMap<String, String> = self
            .themes
            .list()
            .await?
            .themes
            .iter()
            .map(|theme| {
                let record = theme.record().manifest.record();
                (record.id.to_string(), record.version.clone())
            })
            .collect();
        let answers: Vec<(CatalogSource, Result<ParsedIndex, SourceProblem>)> =
            futures_util::stream::iter(sources)
                .map(|source| async move {
                    let result = self.read_index(&source).await;
                    (source, result)
                })
                .buffered(PARALLEL_SOURCES)
                .collect()
                .await;
        let mut themes = Vec::new();
        let mut statuses = Vec::new();
        for (source, result) in answers {
            match result {
                Ok(parsed) => {
                    statuses.push(SourceStatus {
                        source: source.clone(),
                        problem: None,
                        offered: parsed.entries.len(),
                        skipped: parsed.skipped,
                    });
                    for entry in parsed.entries {
                        let availability = Availability::of(
                            installed.get(entry.id.as_str()).map(String::as_str),
                            &entry.version,
                        );
                        themes.push(CatalogTheme {
                            source: source.id().clone(),
                            entry,
                            availability,
                        });
                    }
                }
                Err(problem) => statuses.push(SourceStatus {
                    source,
                    problem: Some(problem),
                    offered: 0,
                    skipped: Vec::new(),
                }),
            }
        }
        themes.sort_by(|a, b| {
            a.entry
                .name
                .to_lowercase()
                .cmp(&b.entry.name.to_lowercase())
                .then_with(|| a.entry.id.cmp(&b.entry.id))
                .then_with(|| a.source.cmp(&b.source))
        });
        Ok(Catalog {
            themes,
            sources: statuses,
        })
    }

    /// Downloads the theme, checks it against the index and installs it like a package.
    pub async fn install(&self, source: &str, theme: &str) -> Result<InstalledTheme, AppError> {
        let id = SourceId::parse(source).ok_or_else(source_not_found)?;
        let theme_id = ThemeId::parse(theme)
            .map_err(|error| AppError::invalid("theme-id-invalid", error.to_string()))?;
        let source = self
            .sources()
            .await?
            .sources
            .into_iter()
            .find(|known| known.id() == &id)
            .ok_or_else(source_not_found)?;
        let parsed = self
            .read_index(&source)
            .await
            .map_err(|problem| match problem.code {
                "catalog-address-refused" | "catalog-too-large" | "catalog-index-invalid" => {
                    AppError::invalid(problem.code, problem.message)
                }
                _ => AppError::unavailable(problem.code, problem.message),
            })?;
        let entry = parsed
            .entries
            .into_iter()
            .find(|entry| entry.id == theme_id)
            .ok_or_else(|| {
                AppError::not_found(
                    "catalog-theme-not-found",
                    format!("the source does not offer {theme}"),
                )
            })?;
        if !entry.compatible {
            return Err(AppError::conflict(
                "catalog-theme-incompatible",
                format!(
                    "{theme} needs SDK {} and does not fit this version",
                    entry.sdk_version
                ),
            ));
        }
        // the address was checked when the index was read; the fetcher checks every hop again
        let bytes = self
            .fetcher
            .fetch(&entry.download_url, MAX_PACKAGE_BYTES)
            .await
            .map_err(|error| fetch_failed(error, "the package"))?;
        if sha256_hex(&bytes) != entry.sha256 {
            return Err(AppError::invalid(
                "catalog-checksum-mismatch",
                "the package does not match the checksum of the index",
            ));
        }
        self.themes
            .install_expecting(bytes, Some((&entry.id, &entry.version)))
            .await
    }

    async fn read_index(&self, source: &CatalogSource) -> Result<ParsedIndex, SourceProblem> {
        // a stored address that the current reach refuses is never fetched
        let url = CatalogUrl::parse(source.url().as_str(), self.reach)
            .map_err(|_| fetch_problem(FetchError::Refused, "the source"))?;
        let bytes = self
            .fetcher
            .fetch(&url, MAX_INDEX_BYTES)
            .await
            .map_err(|error| fetch_problem(error, "the source"))?;
        parse_index(&bytes, self.reach).map_err(|error| SourceProblem {
            code: "catalog-index-invalid",
            message: error.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use super::*;
    use crate::theme_package::tests::package;
    use crate::themes::tests::{archive, fixture};
    use crate::{ChangeOp, Entity, SystemClock};

    #[derive(Default)]
    struct MemorySources {
        stored: Mutex<Option<StoredSources>>,
        feed: Mutex<i64>,
    }

    struct SourcesTx {
        memory: Arc<MemorySources>,
        pending: Option<StoredSources>,
    }

    #[async_trait]
    impl SourcesWrite for SourcesTx {
        async fn sources(&mut self) -> Result<Option<StoredSources>, AppError> {
            Ok(self.memory.stored.lock().clone())
        }

        async fn put(
            &mut self,
            sources: &[CatalogSource],
            expected: Option<Version>,
            _at: Timestamp,
        ) -> Result<(Version, Change), AppError> {
            let current = self
                .memory
                .stored
                .lock()
                .as_ref()
                .map(|stored| stored.version);
            if current != expected {
                return Err(AppError::version_conflict("catalog sources", "list"));
            }
            let version = current.map_or(Version::INITIAL, Version::next);
            self.pending = Some(StoredSources {
                sources: sources.to_vec(),
                version,
            });
            let mut feed = self.memory.feed.lock();
            *feed += 1;
            Ok((
                version,
                Change {
                    seq: *feed,
                    entity: Entity::Setting,
                    entity_id: SOURCES_FEED_ID,
                    op: ChangeOp::Update,
                    at: Timestamp::from_millis(0),
                },
            ))
        }

        async fn commit(self: Box<Self>) -> Result<(), AppError> {
            if let Some(pending) = self.pending {
                *self.memory.stored.lock() = Some(pending);
            }
            Ok(())
        }
    }

    #[async_trait]
    impl SourcesRepository for Arc<MemorySources> {
        async fn begin(&self) -> Result<Box<dyn SourcesWrite>, AppError> {
            Ok(Box::new(SourcesTx {
                memory: self.clone(),
                pending: None,
            }))
        }

        async fn sources(&self) -> Result<Option<StoredSources>, AppError> {
            Ok(self.stored.lock().clone())
        }
    }

    /// Answers by address; a missing answer is unreachable.
    #[derive(Default)]
    struct FakeFetcher {
        answers: Mutex<BTreeMap<String, Result<Vec<u8>, FetchError>>>,
        asked: Mutex<Vec<(String, u64)>>,
    }

    impl FakeFetcher {
        fn answer(&self, url: &str, result: Result<Vec<u8>, FetchError>) {
            self.answers.lock().insert(url.to_owned(), result);
        }
    }

    #[async_trait]
    impl CatalogFetcher for FakeFetcher {
        async fn fetch(&self, url: &CatalogUrl, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
            self.asked.lock().push((url.to_string(), max_bytes));
            self.answers
                .lock()
                .get(url.as_str())
                .cloned()
                .unwrap_or(Err(FetchError::Unreachable))
        }
    }

    struct World {
        service: CatalogService,
        fetcher: Arc<FakeFetcher>,
        themes: Arc<ThemeService>,
        sources: Arc<MemorySources>,
    }

    fn world(reach: Reach) -> World {
        let themes = fixture();
        let themes = Arc::new(themes.service);
        let fetcher = Arc::new(FakeFetcher::default());
        let sources = Arc::new(MemorySources::default());
        let service = CatalogService::new(
            Arc::new(sources.clone()),
            fetcher.clone(),
            themes.clone(),
            Arc::new(SystemClock),
            ChangeSignal::new(),
            reach,
        );
        World {
            service,
            fetcher,
            themes,
            sources,
        }
    }

    const SOURCE: &str = "https://themes.example.com/index.json";
    const PACKAGE_URL: &str = "https://themes.example.com/night-blue.nullradix";

    fn night_blue() -> Vec<u8> {
        archive(&package())
    }

    fn entry(version: &str, bytes: &[u8]) -> Value {
        serde_json::json!({
            "id": "night-blue", "name": "Night Blue", "version": version, "sdkVersion": "^1.3",
            "downloadUrl": PACKAGE_URL, "sha256": sha256_hex(bytes)
        })
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "shaped as an answer of the fetcher"
    )]
    fn index(entries: &[Value]) -> Result<Vec<u8>, FetchError> {
        Ok(serde_json::to_vec(entries).unwrap())
    }

    #[tokio::test]
    async fn sources_are_added_listed_and_removed() {
        let world = world(Reach::Public);
        let empty = world.service.sources().await.unwrap();
        assert!(empty.sources.is_empty() && empty.version.is_none());

        let added = world
            .service
            .add_source(&format!(" {SOURCE}#x "))
            .await
            .unwrap();
        assert_eq!(added.url().as_str(), SOURCE);
        let other = world
            .service
            .add_source("https://other.example.com/i.json")
            .await
            .unwrap();
        let list = world.service.sources().await.unwrap();
        assert_eq!(list.sources, vec![added.clone(), other.clone()]);
        assert_eq!(list.version, Some(Version::from_stored(2)));

        world
            .service
            .remove_source(added.id().as_str(), Version::from_stored(2))
            .await
            .unwrap();
        let list = world.service.sources().await.unwrap();
        assert_eq!(list.sources, vec![other]);
        assert_eq!(list.version, Some(Version::from_stored(3)));
    }

    #[tokio::test]
    async fn a_bad_duplicate_or_surplus_source_is_refused() {
        let world = world(Reach::Public);
        for bad in [
            "",
            "http://themes.example.com/i.json",
            "https://127.0.0.1/i.json",
            "https://u:p@x.example.com/",
        ] {
            let error = world.service.add_source(bad).await.unwrap_err();
            assert_eq!(error.code(), "catalog-source-invalid", "{bad}");
        }
        let first = world.service.add_source(SOURCE).await.unwrap();
        let error = world
            .service
            .add_source("HTTPS://Themes.Example.com/index.json")
            .await
            .unwrap_err();
        assert_eq!(error.code(), "catalog-source-exists");
        assert_eq!(error.existing(), Some(first.id().as_str()));
        for number in 0..MAX_SOURCES {
            let result = world
                .service
                .add_source(&format!("https://s{number}.example.com/i.json"))
                .await;
            if let Err(error) = result {
                assert_eq!(error.code(), "catalog-source-limit");
                assert_eq!(
                    world.service.sources().await.unwrap().sources.len(),
                    MAX_SOURCES
                );
                return;
            }
        }
        panic!("the limit was never reached");
    }

    #[tokio::test]
    async fn removing_needs_the_current_version_and_a_known_source() {
        let world = world(Reach::Public);
        let source = world.service.add_source(SOURCE).await.unwrap();
        let stale = world
            .service
            .remove_source(source.id().as_str(), Version::from_stored(9))
            .await
            .unwrap_err();
        assert_eq!(stale.code(), "version-conflict");
        for id in ["000000000000", "not-an-id"] {
            let error = world
                .service
                .remove_source(id, Version::INITIAL)
                .await
                .unwrap_err();
            assert_eq!(error.code(), "catalog-source-not-found", "{id}");
        }
        assert_eq!(world.service.sources().await.unwrap().sources.len(), 1);
    }

    #[tokio::test]
    async fn browsing_lists_themes_with_their_state_and_skips_bad_entries() {
        let world = world(Reach::Public);
        let bytes = night_blue();
        world.service.add_source(SOURCE).await.unwrap();
        world.fetcher.answer(
            SOURCE,
            index(&[
                entry("1.0.0", &bytes),
                serde_json::json!({ "id": "broken" }),
                serde_json::json!(7),
                entry("1.0.0", &bytes),
            ]),
        );
        let catalog = world.service.browse().await.unwrap();
        assert_eq!(catalog.themes.len(), 1);
        assert_eq!(catalog.themes[0].availability, Availability::NotInstalled);
        let status = &catalog.sources[0];
        assert!(status.problem.is_none());
        assert_eq!(status.offered, 1);
        let reasons: Vec<(usize, Option<&str>)> = status
            .skipped
            .iter()
            .map(|skipped| (skipped.index, skipped.id.as_deref()))
            .collect();
        assert_eq!(
            reasons,
            vec![(1, Some("broken")), (2, None), (3, Some("night-blue"))]
        );
        assert_eq!(
            world.fetcher.asked.lock()[0],
            (SOURCE.to_owned(), MAX_INDEX_BYTES)
        );

        world.themes.install(bytes.clone()).await.unwrap();
        let catalog = world.service.browse().await.unwrap();
        assert_eq!(
            catalog.themes[0].availability,
            Availability::Installed(String::from("1.0.0"))
        );
        world
            .fetcher
            .answer(SOURCE, index(&[entry("1.1.0", &bytes)]));
        let catalog = world.service.browse().await.unwrap();
        assert_eq!(
            catalog.themes[0].availability,
            Availability::Update(String::from("1.0.0"))
        );
    }

    #[tokio::test]
    async fn a_failing_source_does_not_hide_the_others() {
        let world = world(Reach::Public);
        let bytes = night_blue();
        world.service.add_source(SOURCE).await.unwrap();
        world
            .service
            .add_source("https://down.example.com/i.json")
            .await
            .unwrap();
        world
            .service
            .add_source("https://garbage.example.com/i.json")
            .await
            .unwrap();
        world
            .fetcher
            .answer(SOURCE, index(&[entry("1.0.0", &bytes)]));
        world
            .fetcher
            .answer("https://down.example.com/i.json", Err(FetchError::Timeout));
        world
            .fetcher
            .answer("https://garbage.example.com/i.json", Ok(b"{}".to_vec()));
        let catalog = world.service.browse().await.unwrap();
        assert_eq!(catalog.themes.len(), 1);
        let codes: Vec<Option<&str>> = catalog
            .sources
            .iter()
            .map(|status| status.problem.as_ref().map(|problem| problem.code))
            .collect();
        assert_eq!(
            codes,
            vec![None, Some("catalog-timeout"), Some("catalog-index-invalid")]
        );
    }

    #[tokio::test]
    async fn an_index_over_the_bounds_is_refused() {
        let too_many: Vec<Value> = (0..=MAX_INDEX_ENTRIES)
            .map(|number| serde_json::json!({ "id": format!("t{number}") }))
            .collect();
        let bytes = serde_json::to_vec(&too_many).unwrap();
        assert!(parse_index(&bytes, Reach::Public).is_err());
        let huge = vec![b' '; usize::try_from(MAX_INDEX_BYTES).unwrap() + 1];
        assert!(parse_index(&huge, Reach::Public).is_err());
        assert!(parse_index(b"not json", Reach::Public).is_err());
        assert!(parse_index(b"{}", Reach::Public).is_err());
    }

    #[tokio::test]
    async fn a_source_refused_by_the_reach_is_never_fetched() {
        let world = world(Reach::Anywhere);
        world
            .service
            .add_source("http://127.0.0.1:9/i.json")
            .await
            .unwrap();
        let strict = CatalogService::new(
            Arc::new(world.sources.clone()),
            world.fetcher.clone(),
            world.themes.clone(),
            Arc::new(SystemClock),
            ChangeSignal::new(),
            Reach::Public,
        );
        let catalog = strict.browse().await.unwrap();
        assert_eq!(
            catalog.sources[0]
                .problem
                .as_ref()
                .map(|problem| problem.code),
            Some("catalog-address-refused")
        );
        assert!(world.fetcher.asked.lock().is_empty());
    }

    async fn ready(world: &World, bytes: &[u8]) -> String {
        let source = world.service.add_source(SOURCE).await.unwrap();
        world
            .fetcher
            .answer(SOURCE, index(&[entry("1.0.0", bytes)]));
        world.fetcher.answer(PACKAGE_URL, Ok(bytes.to_vec()));
        source.id().to_string()
    }

    #[tokio::test]
    async fn installing_downloads_verifies_and_installs() {
        let world = world(Reach::Public);
        let bytes = night_blue();
        let source = ready(&world, &bytes).await;
        let theme = world.service.install(&source, "night-blue").await.unwrap();
        assert_eq!(theme.id().as_str(), "night-blue");
        assert_eq!(world.themes.list().await.unwrap().themes.len(), 1);
        let asked = world.fetcher.asked.lock().clone();
        assert_eq!(asked[1], (PACKAGE_URL.to_owned(), MAX_PACKAGE_BYTES));
    }

    #[tokio::test]
    async fn a_package_that_does_not_match_the_index_is_not_installed() {
        let world = world(Reach::Public);
        let bytes = night_blue();
        let source = ready(&world, &bytes).await;
        world.fetcher.answer(PACKAGE_URL, Ok(b"tampered".to_vec()));
        let error = world
            .service
            .install(&source, "night-blue")
            .await
            .unwrap_err();
        assert_eq!(error.code(), "catalog-checksum-mismatch");

        // right checksum, but the manifest says another version
        let other = world.fetcher.clone();
        other.answers.lock().clear();
        other.answer(SOURCE, index(&[entry("2.0.0", &bytes)]));
        other.answer(PACKAGE_URL, Ok(bytes.clone()));
        let error = world
            .service
            .install(&source, "night-blue")
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-package-mismatch");
        assert_eq!(world.themes.list().await.unwrap().themes.len(), 0);
    }

    #[tokio::test]
    async fn installing_refuses_unknown_incompatible_and_failing_downloads() {
        let world = world(Reach::Public);
        let bytes = night_blue();
        let source = ready(&world, &bytes).await;
        let error = world
            .service
            .install(&source, "ghost-theme")
            .await
            .unwrap_err();
        assert_eq!(error.code(), "catalog-theme-not-found");
        let error = world
            .service
            .install("111111111111", "night-blue")
            .await
            .unwrap_err();
        assert_eq!(error.code(), "catalog-source-not-found");
        let error = world.service.install(&source, "Bad Id").await.unwrap_err();
        assert_eq!(error.code(), "theme-id-invalid");

        let mut old = entry("1.0.0", &bytes);
        old["sdkVersion"] = Value::from("^9.0");
        world.fetcher.answers.lock().clear();
        world.fetcher.answer(SOURCE, index(&[old]));
        let error = world
            .service
            .install(&source, "night-blue")
            .await
            .unwrap_err();
        assert_eq!(error.code(), "catalog-theme-incompatible");

        for (failure, code) in [
            (FetchError::Timeout, "catalog-timeout"),
            (FetchError::TooLarge, "catalog-too-large"),
            (FetchError::Status(404), "catalog-http-error"),
            (FetchError::Refused, "catalog-address-refused"),
        ] {
            world.fetcher.answers.lock().clear();
            world
                .fetcher
                .answer(SOURCE, index(&[entry("1.0.0", &bytes)]));
            world.fetcher.answer(PACKAGE_URL, Err(failure));
            let error = world
                .service
                .install(&source, "night-blue")
                .await
                .unwrap_err();
            assert_eq!(error.code(), code);
        }
        assert_eq!(world.themes.list().await.unwrap().themes.len(), 0);
    }

    #[test]
    fn stored_sources_round_trip() {
        let url = CatalogUrl::parse(SOURCE, Reach::Public).unwrap();
        let sources = vec![CatalogSource::new(url, Timestamp::from_millis(5))];
        let text = sources_json(&sources).to_string();
        assert_eq!(parse_sources(&text), Some(sources));
        assert_eq!(parse_sources("{}"), None);
        assert_eq!(parse_sources(r#"[{"url":"https://a.example.com/"}]"#), None);
    }
}
