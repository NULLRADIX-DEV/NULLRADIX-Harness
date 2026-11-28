//! The asset proxy: a theme with its code on loads resources only from the domains it declares.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_domain::{
    AssetUrl, AssetUrlError, CatalogUrl, MAX_ARCHIVE_BYTES, MAX_ASSET_BYTES, MAX_REDIRECTS,
    ThemeId, check_asset_entry, foreign_content_type, parse_sha256,
};
use sha2::{Digest, Sha256};

use crate::flights::Flights;
use crate::theme_catalog::FetchError;
use crate::themes::{AssetGrant, ThemeService};
use crate::{AppError, auth};

/// The whole download of one resource, across redirects.
const DOWNLOAD_BUDGET: Duration = Duration::from_secs(120);

/// What one request to a host brought; adapters never follow redirects themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hop {
    Body(Vec<u8>),
    /// Where the host sends the request next; a relative address is possible.
    Redirect(String),
}

/// Makes one request; adapters enforce https, public addresses on every connection, time and size.
#[async_trait]
pub trait AssetFetcher: Send + Sync {
    /// The answer of the host; a body of more than `max_bytes` fails with `TooLarge`.
    async fn get(&self, url: &CatalogUrl, max_bytes: u64) -> Result<Hop, FetchError>;
}

/// Fetcher for hosts that cannot reach the network.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAssetNetwork;

#[async_trait]
impl AssetFetcher for NoAssetNetwork {
    async fn get(&self, _url: &CatalogUrl, _max_bytes: u64) -> Result<Hop, FetchError> {
        Err(FetchError::Unreachable)
    }
}

/// Why a file does not come out of an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryError {
    NotArchive,
    NotFound,
    /// A folder, a link or a name that is not plain.
    Unsafe,
    TooLarge,
}

/// Reads one file out of a zip archive held in memory.
#[async_trait]
pub trait ZipEntries: Send + Sync {
    /// The file at `path`; more than `max_bytes` fails with `TooLarge`.
    async fn entry(
        &self,
        archive: Vec<u8>,
        path: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, EntryError>;
}

/// Archives for hosts that cannot read them.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoZipEntries;

#[async_trait]
impl ZipEntries for NoZipEntries {
    async fn entry(
        &self,
        _archive: Vec<u8>,
        _path: &str,
        _max_bytes: u64,
    ) -> Result<Vec<u8>, EntryError> {
        Err(EntryError::NotArchive)
    }
}

/// The bytes handed to a cache come back, so they are served without a copy.
#[derive(Debug)]
pub struct Stored {
    pub bytes: Vec<u8>,
    /// Why the cache could not keep them.
    pub error: Option<AppError>,
}

/// Bytes kept per theme under a key; adapters keep the total below a budget.
#[async_trait]
pub trait AssetCache: Send + Sync {
    /// A miss and an unreadable entry both give `None`.
    async fn get(&self, theme: &ThemeId, key: &str) -> Option<Vec<u8>>;
    async fn put(&self, theme: &ThemeId, key: &str, bytes: Vec<u8>) -> Result<Stored, AppError>;
}

/// A cache that keeps nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAssetCache;

#[async_trait]
impl AssetCache for NoAssetCache {
    async fn get(&self, _theme: &ThemeId, _key: &str) -> Option<Vec<u8>> {
        None
    }

    async fn put(&self, _theme: &ThemeId, _key: &str, bytes: Vec<u8>) -> Result<Stored, AppError> {
        Ok(Stored { bytes, error: None })
    }
}

/// Downloads that run at the same time, all themes together.
pub const MAX_PARALLEL_DOWNLOADS: usize = 4;

/// Archives read from the cache or downloaded at the same time, all themes together.
pub const MAX_PARALLEL_ARCHIVES: usize = 4;

type FlightKey = (ThemeId, String);

/// What a page asks the proxy for.
#[derive(Debug, Clone, Copy)]
pub struct AssetRequest<'a> {
    pub theme: &'a str,
    pub url: &'a str,
    /// Checksum of the resource, an archive when `entry` is set.
    pub sha256: Option<&'a str>,
    /// File inside the archive at `url`.
    pub entry: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub data: Vec<u8>,
    pub content_type: &'static str,
}

fn forbidden(message: impl Into<String>) -> AppError {
    AppError::forbidden("theme-fetch-forbidden", message)
}

/// The code of the theme is not on, so nothing it asks for is loaded.
#[must_use]
pub fn code_is_off() -> AppError {
    forbidden("the code switch of the theme is off")
}

fn url_invalid(error: &AssetUrlError) -> AppError {
    match error {
        AssetUrlError::Invalid(error) => {
            AppError::invalid("theme-fetch-url-invalid", error.to_string())
        }
        AssetUrlError::NotDeclared { .. } => forbidden(error.to_string()),
    }
}

fn redirect_refused(error: &AssetUrlError) -> AppError {
    match error {
        AssetUrlError::Invalid(_) => AppError::forbidden(
            "theme-fetch-redirect-refused",
            "the host redirects to an address that is not allowed",
        ),
        AssetUrlError::NotDeclared { .. } => forbidden(format!("redirect: {error}")),
    }
}

fn too_large(max: u64) -> AppError {
    AppError::invalid(
        "theme-fetch-too-large",
        format!("the resource is larger than {max} bytes"),
    )
}

fn fetch_failed(error: FetchError, max: u64) -> AppError {
    match error {
        FetchError::Refused => AppError::forbidden(
            "theme-fetch-address-refused",
            "the host resolves to an address that is not public",
        ),
        FetchError::Timeout => {
            AppError::unavailable("theme-fetch-timeout", "the host did not answer in time")
        }
        FetchError::Unreachable => {
            AppError::unavailable("theme-fetch-unreachable", "the host cannot be reached")
        }
        FetchError::Status(status) => AppError::unavailable(
            "theme-fetch-upstream-status",
            format!("the host answered with HTTP {status}"),
        ),
        FetchError::TooLarge => too_large(max),
    }
}

fn entry_failed(error: EntryError) -> AppError {
    match error {
        EntryError::NotArchive => {
            AppError::unavailable("theme-fetch-not-archive", "the resource is no zip archive")
        }
        EntryError::NotFound => AppError::not_found(
            "theme-fetch-entry-not-found",
            "the archive has no such file",
        ),
        EntryError::Unsafe => {
            AppError::invalid("theme-fetch-entry-invalid", "the entry is not a plain file")
        }
        EntryError::TooLarge => too_large(MAX_ASSET_BYTES),
    }
}

/// The checksum of `bytes`, hashed off the async runtime; the bytes come back with it.
async fn checksum(bytes: Vec<u8>) -> Result<(String, Vec<u8>), AppError> {
    tokio::task::spawn_blocking(move || (auth::hex(&Sha256::digest(&bytes)), bytes))
        .await
        .map_err(AppError::internal)
}

fn cache_key(parts: &[&str]) -> String {
    auth::hex(&Sha256::digest(parts.join("\n").as_bytes()))
}

pub struct ThemeAssets {
    themes: Arc<ThemeService>,
    fetcher: Arc<dyn AssetFetcher>,
    entries: Arc<dyn ZipEntries>,
    cache: Arc<dyn AssetCache>,
    flights: Flights<FlightKey>,
    downloads: tokio::sync::Semaphore,
    archives: tokio::sync::Semaphore,
}

impl std::fmt::Debug for ThemeAssets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThemeAssets").finish_non_exhaustive()
    }
}

impl ThemeAssets {
    #[must_use]
    pub fn new(
        themes: Arc<ThemeService>,
        fetcher: Arc<dyn AssetFetcher>,
        entries: Arc<dyn ZipEntries>,
        cache: Arc<dyn AssetCache>,
    ) -> Self {
        Self {
            themes,
            fetcher,
            entries,
            cache,
            flights: Flights::default(),
            downloads: tokio::sync::Semaphore::new(MAX_PARALLEL_DOWNLOADS),
            archives: tokio::sync::Semaphore::new(MAX_PARALLEL_ARCHIVES),
        }
    }

    /// A resource, or a file of an archive, from a domain the installed theme declares.
    pub async fn fetch(&self, request: AssetRequest<'_>) -> Result<Asset, AppError> {
        let grant = self.themes.asset_grant(request.theme).await?;
        let sum = request
            .sha256
            .map(|text| {
                parse_sha256(text).ok_or_else(|| {
                    AppError::invalid(
                        "theme-fetch-checksum-invalid",
                        "sha256 is 64 hexadecimal digits",
                    )
                })
            })
            .transpose()?;
        if let Some(entry) = request.entry {
            check_asset_entry(entry).map_err(|error| {
                AppError::invalid("theme-fetch-entry-invalid", error.to_string())
            })?;
        }
        let url =
            AssetUrl::parse(request.url, &grant.network).map_err(|error| url_invalid(&error))?;
        let named = request.entry.map_or_else(|| url.path(), str::to_owned);
        let data = match request.entry {
            Some(entry) => self.entry(&grant, &url, sum.as_deref(), entry).await?,
            None => {
                self.resource(&grant, &url, sum.as_deref(), MAX_ASSET_BYTES)
                    .await?
            }
        };
        Ok(Asset {
            data,
            content_type: foreign_content_type(&named),
        })
    }

    /// The file of an archive; the archive is cached too, so more files cost no download.
    async fn entry(
        &self,
        grant: &AssetGrant,
        url: &AssetUrl,
        sum: Option<&str>,
        entry: &str,
    ) -> Result<Vec<u8>, AppError> {
        // the checksum is of the archive, so a file is kept for the checksum it was taken under
        let key = cache_key(&[url.as_str(), entry, sum.unwrap_or_default()]);
        if let Some(cached) = self.cached_entry(&grant.id, &key).await {
            return Ok(cached);
        }
        let archive_key = cache_key(&[url.as_str()]);
        // an archive is read whole, so only a few are read, downloaded or cut open at once
        let permit = self.archives.acquire().await.map_err(AppError::internal)?;
        // whoever was ahead may have cut the file out meanwhile
        if let Some(cached) = self.cached_entry(&grant.id, &key).await {
            return Ok(cached);
        }
        // a cached archive needs no flight, so cold reads of its files run side by side
        let cached = self
            .cached(grant, &archive_key, sum, MAX_ARCHIVE_BYTES)
            .await?;
        let (archive, _permit) = if let Some(archive) = cached {
            (archive, permit)
        } else {
            // never wait for a flight while holding a permit: a slow download would block all
            drop(permit);
            let flight = self
                .flights
                .enter((grant.id.clone(), archive_key.clone()))
                .await;
            let permit = self.archives.acquire().await.map_err(AppError::internal)?;
            // the flight ahead may have cut this file meanwhile
            if let Some(cached) = self.cached_entry(&grant.id, &key).await {
                return Ok(cached);
            }
            let archive = self
                .load(grant, url, sum, MAX_ARCHIVE_BYTES, &archive_key)
                .await?;
            // the bytes are in hand, so the next request may read the archive while this one cuts
            drop(flight);
            (archive, permit)
        };
        let bytes = self
            .entries
            .entry(archive, entry, MAX_ASSET_BYTES)
            .await
            .map_err(entry_failed)?;
        if bytes.len() as u64 > MAX_ASSET_BYTES {
            return Err(too_large(MAX_ASSET_BYTES));
        }
        self.keep(grant, &key, bytes).await
    }

    async fn cached_entry(&self, theme: &ThemeId, key: &str) -> Option<Vec<u8>> {
        let cached = self.cache.get(theme, key).await?;
        (cached.len() as u64 <= MAX_ASSET_BYTES).then_some(cached)
    }

    /// The cached resource, if it fits `max` and has the checksum.
    async fn cached(
        &self,
        grant: &AssetGrant,
        key: &str,
        sum: Option<&str>,
        max: u64,
    ) -> Result<Option<Vec<u8>>, AppError> {
        let Some(cached) = self.cache.get(&grant.id, key).await else {
            return Ok(None);
        };
        if cached.len() as u64 > max {
            return Ok(None);
        }
        let Some(sum) = sum else {
            return Ok(Some(cached));
        };
        let (found, cached) = checksum(cached).await?;
        // a stale entry is replaced by what the host serves now
        Ok((found == sum).then_some(cached))
    }

    async fn resource(
        &self,
        grant: &AssetGrant,
        url: &AssetUrl,
        sum: Option<&str>,
        max: u64,
    ) -> Result<Vec<u8>, AppError> {
        let key = cache_key(&[url.as_str()]);
        if let Some(bytes) = self.cached(grant, &key, sum, max).await? {
            return Ok(bytes);
        }
        let _flight = self.flights.enter((grant.id.clone(), key.clone())).await;
        self.load(grant, url, sum, max, &key).await
    }

    /// The resource from the cache or the host; the caller holds the flight of `key`.
    async fn load(
        &self,
        grant: &AssetGrant,
        url: &AssetUrl,
        sum: Option<&str>,
        max: u64,
        key: &str,
    ) -> Result<Vec<u8>, AppError> {
        // whoever was ahead may have loaded it meanwhile
        if let Some(bytes) = self.cached(grant, key, sum, max).await? {
            return Ok(bytes);
        }
        let mut bytes = {
            let _permit = self.downloads.acquire().await.map_err(AppError::internal)?;
            self.download(grant, url, max).await?
        };
        if let Some(sum) = sum {
            let (found, hashed) = checksum(bytes).await?;
            if found != sum {
                return Err(AppError::invalid(
                    "theme-fetch-checksum-mismatch",
                    "the resource does not have the expected checksum",
                ));
            }
            bytes = hashed;
        }
        self.keep(grant, key, bytes).await
    }

    /// Follows redirects, each one checked against the domains of the theme.
    async fn download(
        &self,
        grant: &AssetGrant,
        first: &AssetUrl,
        max: u64,
    ) -> Result<Vec<u8>, AppError> {
        let work = async {
            let mut url = first.clone();
            for _ in 0..=MAX_REDIRECTS {
                let hop = self
                    .fetcher
                    .get(url.catalog_url(), max)
                    .await
                    .map_err(|error| fetch_failed(error, max))?;
                match hop {
                    Hop::Body(bytes) if bytes.len() as u64 > max => return Err(too_large(max)),
                    Hop::Body(bytes) => return Ok(bytes),
                    Hop::Redirect(location) => {
                        url = url
                            .redirect(&location, &grant.network)
                            .map_err(|error| redirect_refused(&error))?;
                    }
                }
            }
            Err(AppError::unavailable(
                "theme-fetch-redirects",
                format!("the host redirects more than {MAX_REDIRECTS} times"),
            ))
        };
        let bytes = tokio::time::timeout(DOWNLOAD_BUDGET, work)
            .await
            .map_err(|_| fetch_failed(FetchError::Timeout, max))??;
        // the person may have turned the code off, or an update replaced the theme, meanwhile
        self.themes.check_grant(grant).await?;
        Ok(bytes)
    }

    /// Hands the bytes to the cache and takes them back; a cache that cannot write is only logged.
    async fn keep(
        &self,
        grant: &AssetGrant,
        key: &str,
        bytes: Vec<u8>,
    ) -> Result<Vec<u8>, AppError> {
        let stored = self.cache.put(&grant.id, key, bytes).await?;
        if let Some(error) = stored.error {
            tracing::warn!(%error, theme = %grant.id, "cannot cache a theme asset");
        }
        Ok(stored.bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use parking_lot::Mutex;

    use super::*;
    use crate::theme_package::tests::{MANIFEST, file};
    use crate::themes::tests::{Fixture, archive, fixture};

    const NETWORK: &str = r#""network": ["cdn.example.com", "*.assets.test"]"#;
    const ICON: &str = "https://cdn.example.com/icon.png";

    /// A semaphore that starts without permits.
    struct Permits(tokio::sync::Semaphore);

    impl Default for Permits {
        fn default() -> Self {
            Self(tokio::sync::Semaphore::new(0))
        }
    }

    impl Permits {
        /// Waits for one arrival; a rule that never lets it come fails the test.
        async fn arrived(&self) {
            let permit = tokio::time::timeout(Duration::from_secs(5), self.0.acquire())
                .await
                .expect("nothing reached the gate within 5 seconds");
            permit.unwrap().forget();
        }
    }

    impl std::ops::Deref for Permits {
        type Target = tokio::sync::Semaphore;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    /// Answers a url with its scripted hops in turn and records every request.
    #[derive(Default)]
    struct Hosts {
        answers: Mutex<HashMap<String, Vec<Result<Hop, FetchError>>>>,
        asked: Mutex<Vec<(String, u64)>>,
        /// When set, a request waits to be released before it answers.
        gate: Mutex<bool>,
        /// One permit per request that arrived at the gate.
        entered: Permits,
        /// One permit lets one request through.
        release: Permits,
        inflight: AtomicUsize,
        peak: AtomicUsize,
        silent: Mutex<bool>,
    }

    impl Hosts {
        fn serve(&self, url: &str, hops: Vec<Result<Hop, FetchError>>) {
            self.answers.lock().insert(url.to_owned(), hops);
        }

        fn body(&self, url: &str, bytes: &[u8]) {
            self.serve(url, vec![Ok(Hop::Body(bytes.to_vec()))]);
        }

        fn asked(&self) -> Vec<String> {
            self.asked
                .lock()
                .iter()
                .map(|(url, _)| url.clone())
                .collect()
        }
    }

    #[async_trait]
    impl AssetFetcher for Hosts {
        async fn get(&self, url: &CatalogUrl, max_bytes: u64) -> Result<Hop, FetchError> {
            self.asked.lock().push((url.to_string(), max_bytes));
            if *self.silent.lock() {
                std::future::pending::<()>().await;
            }
            let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            let held = *self.gate.lock();
            if held {
                self.entered.add_permits(1);
                self.release.acquire().await.unwrap().forget();
            }
            self.inflight.fetch_sub(1, Ordering::SeqCst);
            let mut answers = self.answers.lock();
            match answers.get_mut(url.as_str()) {
                Some(hops) if !hops.is_empty() => hops.remove(0),
                _ => Err(FetchError::Unreachable),
            }
        }
    }

    /// Archives are `name=text` lines.
    struct Lines;

    #[async_trait]
    impl ZipEntries for Lines {
        async fn entry(
            &self,
            archive: Vec<u8>,
            path: &str,
            max_bytes: u64,
        ) -> Result<Vec<u8>, EntryError> {
            let text = String::from_utf8(archive).map_err(|_| EntryError::NotArchive)?;
            if !text.contains('=') {
                return Err(EntryError::NotArchive);
            }
            let found = text
                .lines()
                .filter_map(|line| line.split_once('='))
                .find(|(name, _)| *name == path)
                .map(|(_, data)| data.as_bytes().to_vec())
                .ok_or(EntryError::NotFound)?;
            if found.len() as u64 > max_bytes {
                return Err(EntryError::TooLarge);
            }
            Ok(found)
        }
    }

    /// `Lines` that holds every call at a gate and records how many run at once.
    #[derive(Default)]
    struct Gated {
        entered: Permits,
        release: Permits,
        inflight: AtomicUsize,
        peak: AtomicUsize,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ZipEntries for Gated {
        async fn entry(
            &self,
            archive: Vec<u8>,
            path: &str,
            max_bytes: u64,
        ) -> Result<Vec<u8>, EntryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
            self.inflight.fetch_sub(1, Ordering::SeqCst);
            Lines.entry(archive, path, max_bytes).await
        }
    }

    /// Every file of an archive is a folder or a link.
    struct Unsafe;

    #[async_trait]
    impl ZipEntries for Unsafe {
        async fn entry(&self, _: Vec<u8>, _: &str, _: u64) -> Result<Vec<u8>, EntryError> {
            Err(EntryError::Unsafe)
        }
    }

    #[derive(Default)]
    struct Memory {
        kept: Mutex<BTreeMap<(String, String), Vec<u8>>>,
        fail_put: Mutex<bool>,
    }

    #[async_trait]
    impl AssetCache for Memory {
        async fn get(&self, theme: &ThemeId, key: &str) -> Option<Vec<u8>> {
            self.kept
                .lock()
                .get(&(theme.to_string(), key.to_owned()))
                .cloned()
        }

        async fn put(
            &self,
            theme: &ThemeId,
            key: &str,
            bytes: Vec<u8>,
        ) -> Result<Stored, AppError> {
            if *self.fail_put.lock() {
                let error = AppError::unavailable("cache-failed", "disk");
                return Ok(Stored {
                    bytes,
                    error: Some(error),
                });
            }
            self.kept
                .lock()
                .insert((theme.to_string(), key.to_owned()), bytes.clone());
            Ok(Stored { bytes, error: None })
        }
    }

    /// A memory cache that holds reads of the listed keys at a gate and counts them.
    #[derive(Default)]
    struct SlowReads {
        memory: Memory,
        slow: Mutex<Vec<String>>,
        entered: Permits,
        release: Permits,
        inflight: AtomicUsize,
        peak: AtomicUsize,
    }

    #[async_trait]
    impl AssetCache for SlowReads {
        async fn get(&self, theme: &ThemeId, key: &str) -> Option<Vec<u8>> {
            let held = self.slow.lock().iter().any(|slow| slow == key);
            if held {
                let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(now, Ordering::SeqCst);
                self.entered.add_permits(1);
                self.release.acquire().await.unwrap().forget();
                self.inflight.fetch_sub(1, Ordering::SeqCst);
            }
            self.memory.get(theme, key).await
        }

        async fn put(
            &self,
            theme: &ThemeId,
            key: &str,
            bytes: Vec<u8>,
        ) -> Result<Stored, AppError> {
            self.memory.put(theme, key, bytes).await
        }
    }

    /// The cache key of the archive at `url`, which must be on a declared domain.
    fn archive_key(url: &str) -> String {
        let url = AssetUrl::parse(url, &["cdn.example.com".to_owned()]).unwrap();
        cache_key(&[url.as_str()])
    }

    struct Setup {
        fixture: Fixture,
        hosts: Arc<Hosts>,
        cache: Arc<Memory>,
        assets: Arc<ThemeAssets>,
    }

    fn code_package(version: &str, network: &str) -> Vec<u8> {
        let manifest = MANIFEST
            .replace("1.0.0", version)
            .replace(r#""network": []"#, network);
        archive(&[
            file("manifest.json", &manifest),
            file("theme.css", "a { color: red }"),
            file("main.js", "run()"),
        ])
    }

    /// The theme `night-blue` is installed with code and the domains above; its switch is `on`.
    async fn setup(on: bool) -> Setup {
        let fixture = fixture();
        fixture
            .service
            .install(code_package("1.0.0", NETWORK), true, true)
            .await
            .unwrap();
        if on {
            fixture
                .service
                .set_code("night-blue", true, Some(nullradix_domain::Version::INITIAL))
                .await
                .unwrap();
        }
        let hosts = Arc::new(Hosts::default());
        let cache = Arc::new(Memory::default());
        let assets = Arc::new(ThemeAssets::new(
            fixture.service.clone(),
            hosts.clone(),
            Arc::new(Lines),
            cache.clone(),
        ));
        Setup {
            fixture,
            hosts,
            cache,
            assets,
        }
    }

    fn request(url: &str) -> AssetRequest<'_> {
        AssetRequest {
            theme: "night-blue",
            url,
            sha256: None,
            entry: None,
        }
    }

    fn entry_of<'a>(url: &'a str, entry: &'a str) -> AssetRequest<'a> {
        AssetRequest {
            entry: Some(entry),
            ..request(url)
        }
    }

    async fn code_of(setup: &Setup, request: AssetRequest<'_>) -> &'static str {
        setup.assets.fetch(request).await.unwrap_err().code()
    }

    fn sum(bytes: &[u8]) -> String {
        auth::hex(&Sha256::digest(bytes))
    }

    #[tokio::test]
    async fn a_declared_resource_is_fetched_typed_by_its_extension_and_cached() {
        let setup = setup(true).await;
        setup.hosts.body(ICON, b"PNG");
        let asset = setup.assets.fetch(request(ICON)).await.unwrap();
        assert_eq!(
            (asset.data.as_slice(), asset.content_type),
            (b"PNG".as_slice(), "image/png")
        );
        assert_eq!(setup.hosts.asked(), [ICON]);
        assert_eq!(setup.cache.kept.lock().len(), 1);
        // the host is gone, the bytes are not
        let again = setup.assets.fetch(request(ICON)).await.unwrap();
        assert_eq!(again, asset);
        assert_eq!(setup.hosts.asked().len(), 1);
    }

    #[tokio::test]
    async fn scripts_styles_and_unknown_types_are_served_as_opaque_bytes() {
        let setup = setup(true).await;
        for name in ["run.js", "look.css", "page.html", "plain"] {
            let url = format!("https://cdn.example.com/{name}");
            setup.hosts.body(&url, b"x");
            let asset = setup.assets.fetch(request(&url)).await.unwrap();
            assert_eq!(asset.content_type, "application/octet-stream", "{name}");
        }
    }

    #[tokio::test]
    async fn only_an_installed_theme_with_its_code_on_may_use_the_proxy() {
        let off = setup(false).await;
        off.hosts.body(ICON, b"PNG");
        let error = off.assets.fetch(request(ICON)).await.unwrap_err();
        assert_eq!(error.code(), "theme-fetch-forbidden");
        assert_eq!(error.kind(), crate::ErrorKind::Forbidden);
        for theme in ["other-theme", "Night Blue", ""] {
            let error = off
                .assets
                .fetch(AssetRequest {
                    theme,
                    ..request(ICON)
                })
                .await
                .unwrap_err();
            assert_eq!(error.code(), "theme-not-found", "{theme:?}");
        }
        assert!(off.hosts.asked().is_empty() && off.cache.kept.lock().is_empty());

        off.fixture
            .service
            .set_code("night-blue", true, Some(nullradix_domain::Version::INITIAL))
            .await
            .unwrap();
        assert!(off.assets.fetch(request(ICON)).await.is_ok());
        off.fixture
            .service
            .set_code("night-blue", false, None)
            .await
            .unwrap();
        // what is cached is not served either
        assert_eq!(code_of(&off, request(ICON)).await, "theme-fetch-forbidden");
        assert_eq!(off.hosts.asked().len(), 1);
    }

    #[tokio::test]
    async fn a_host_the_theme_does_not_declare_is_refused_before_any_request() {
        let setup = setup(true).await;
        for url in [
            "https://other.example.com/a.png",
            "https://cdn.example.com.evil.test/a.png",
            "https://assets.test/a.png",
            "https://cdn.ex\u{430}mple.com/a.png",
            "https://\u{441}dn.example.com/a.png",
        ] {
            assert_eq!(
                code_of(&setup, request(url)).await,
                "theme-fetch-forbidden",
                "{url}"
            );
        }
        for url in [
            "http://cdn.example.com/a.png",
            "https://cdn.example.com:8443/a.png",
            "https://user@cdn.example.com/a.png",
            "file:///etc/passwd",
            "",
        ] {
            assert_eq!(
                code_of(&setup, request(url)).await,
                "theme-fetch-url-invalid",
                "{url:?}"
            );
        }
        assert_eq!(setup.hosts.asked(), Vec::<String>::new());
        assert!(setup.cache.kept.lock().is_empty());
    }

    #[tokio::test]
    async fn a_cached_resource_of_a_host_the_theme_no_longer_declares_is_not_served() {
        let setup = setup(true).await;
        setup.hosts.body(ICON, b"PNG");
        setup.assets.fetch(request(ICON)).await.unwrap();
        // the person's update drops the domain and keeps the code on
        let narrower = code_package("1.0.1", r#""network": ["other.example.com"]"#);
        setup
            .fixture
            .service
            .install(narrower, true, true)
            .await
            .unwrap();
        setup
            .fixture
            .service
            .set_code(
                "night-blue",
                true,
                Some(nullradix_domain::Version::from_stored(2)),
            )
            .await
            .unwrap();
        assert_eq!(
            code_of(&setup, request(ICON)).await,
            "theme-fetch-forbidden"
        );
    }

    #[tokio::test]
    async fn every_redirect_is_checked_against_the_domains_of_the_theme() {
        let setup = setup(true).await;
        let moved = |to: &str| vec![Ok(Hop::Redirect(to.to_owned()))];
        setup.hosts.serve(ICON, moved("/other/icon.png"));
        setup
            .hosts
            .body("https://cdn.example.com/other/icon.png", b"PNG");
        let asset = setup.assets.fetch(request(ICON)).await.unwrap();
        assert_eq!(asset.data, b"PNG");
        assert_eq!(setup.hosts.asked().len(), 2);

        let evil = "https://cdn.example.com/evil";
        for (target, code) in [
            ("https://evil.test/x.png", "theme-fetch-forbidden"),
            (
                "http://cdn.example.com/x.png",
                "theme-fetch-redirect-refused",
            ),
            ("https://127.0.0.1/x.png", "theme-fetch-redirect-refused"),
            ("https://[::1]/x.png", "theme-fetch-redirect-refused"),
            (
                "https://cdn.example.com:8443/x.png",
                "theme-fetch-redirect-refused",
            ),
            (
                "https://user:pw@cdn.example.com/x.png",
                "theme-fetch-redirect-refused",
            ),
            // a Cyrillic a in place of the Latin one is another host
            (
                "https://cdn.ex\u{430}mple.com/x.png",
                "theme-fetch-forbidden",
            ),
        ] {
            setup.hosts.serve(evil, moved(target));
            setup.hosts.body(target, b"secret");
            let before = setup.hosts.asked().len();
            assert_eq!(code_of(&setup, request(evil)).await, code, "{target}");
            assert_eq!(setup.hosts.asked().len(), before + 1, "{target}");
        }
        assert_eq!(setup.cache.kept.lock().len(), 1);
    }

    #[tokio::test]
    async fn at_most_five_redirects_are_followed() {
        let setup = setup(true).await;
        let chain = |name: &str, count: usize| {
            for hop in 0..count {
                let from = format!("https://cdn.example.com/{name}/{hop}");
                let to = format!("https://cdn.example.com/{name}/{}", hop + 1);
                setup.hosts.serve(&from, vec![Ok(Hop::Redirect(to))]);
            }
            setup
                .hosts
                .body(&format!("https://cdn.example.com/{name}/{count}"), b"end");
        };
        chain("ok", MAX_REDIRECTS);
        let reached = setup
            .assets
            .fetch(request("https://cdn.example.com/ok/0"))
            .await;
        assert_eq!(reached.unwrap().data, b"end");
        assert_eq!(setup.hosts.asked().len(), MAX_REDIRECTS + 1);

        chain("far", MAX_REDIRECTS + 1);
        let before = setup.hosts.asked().len();
        let error = setup
            .assets
            .fetch(request("https://cdn.example.com/far/0"))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-fetch-redirects");
        assert_eq!(setup.hosts.asked().len(), before + MAX_REDIRECTS + 1);
    }

    #[tokio::test]
    async fn a_host_that_resolves_to_a_private_address_is_refused_on_every_hop() {
        let setup = setup(true).await;
        setup.hosts.serve(ICON, vec![Err(FetchError::Refused)]);
        let error = setup.assets.fetch(request(ICON)).await.unwrap_err();
        assert_eq!(
            (error.code(), error.kind()),
            ("theme-fetch-address-refused", crate::ErrorKind::Forbidden)
        );
        // the second hop is the one that resolves badly
        setup.hosts.serve(
            ICON,
            vec![Ok(Hop::Redirect(String::from(
                "https://x.assets.test/b.png",
            )))],
        );
        setup.hosts.serve(
            "https://x.assets.test/b.png",
            vec![Err(FetchError::Refused)],
        );
        assert_eq!(
            code_of(&setup, request(ICON)).await,
            "theme-fetch-address-refused"
        );
        assert!(setup.cache.kept.lock().is_empty());
    }

    #[tokio::test]
    async fn upstream_failures_have_codes_of_their_own() {
        let setup = setup(true).await;
        for (failure, code) in [
            (FetchError::Timeout, "theme-fetch-timeout"),
            (FetchError::Unreachable, "theme-fetch-unreachable"),
            (FetchError::Status(404), "theme-fetch-upstream-status"),
            (FetchError::TooLarge, "theme-fetch-too-large"),
        ] {
            setup.hosts.serve(ICON, vec![Err(failure)]);
            assert_eq!(code_of(&setup, request(ICON)).await, code);
        }
        assert!(setup.cache.kept.lock().is_empty());
    }

    #[tokio::test]
    async fn a_resource_just_above_the_cap_is_refused_and_one_at_the_cap_is_served() {
        let setup = setup(true).await;
        let cap = usize::try_from(MAX_ASSET_BYTES).unwrap();
        setup.hosts.body(ICON, &vec![0; cap]);
        let asset = setup.assets.fetch(request(ICON)).await.unwrap();
        assert_eq!(asset.data.len(), cap);
        assert_eq!(setup.hosts.asked.lock()[0].1, MAX_ASSET_BYTES);

        let other = "https://cdn.example.com/big.png";
        setup.hosts.body(other, &vec![0; cap + 1]);
        assert_eq!(
            code_of(&setup, request(other)).await,
            "theme-fetch-too-large"
        );
        assert_eq!(setup.cache.kept.lock().len(), 1);
    }

    #[tokio::test]
    async fn an_archive_just_above_the_cap_is_refused_and_a_file_just_above_its_cap_too() {
        let setup = setup(true).await;
        let jar = "https://cdn.example.com/client.jar";
        let cap = usize::try_from(MAX_ARCHIVE_BYTES).unwrap();
        setup.hosts.body(jar, &vec![b'x'; cap + 1]);
        assert_eq!(
            code_of(&setup, entry_of(jar, "a.png")).await,
            "theme-fetch-too-large"
        );
        assert_eq!(setup.hosts.asked.lock()[0].1, MAX_ARCHIVE_BYTES);
        assert!(setup.cache.kept.lock().is_empty());

        let mut small = b"a.png=".to_vec();
        small.extend(vec![b'x'; usize::try_from(MAX_ASSET_BYTES).unwrap() + 1]);
        setup.hosts.body(jar, &small);
        assert_eq!(
            code_of(&setup, entry_of(jar, "a.png")).await,
            "theme-fetch-too-large"
        );
        // only the archive is kept, not a file that was refused
        assert_eq!(setup.cache.kept.lock().len(), 1);
    }

    #[tokio::test]
    async fn a_checksum_is_checked_and_a_wrong_one_caches_nothing() {
        let setup = setup(true).await;
        for bad in ["abc", "zz", &"g".repeat(64)] {
            assert_eq!(
                code_of(
                    &setup,
                    AssetRequest {
                        sha256: Some(bad),
                        ..request(ICON)
                    }
                )
                .await,
                "theme-fetch-checksum-invalid"
            );
        }
        assert_eq!(setup.hosts.asked(), Vec::<String>::new());

        setup.hosts.body(ICON, b"PNG");
        let wrong = "0".repeat(64);
        let error = setup
            .assets
            .fetch(AssetRequest {
                sha256: Some(&wrong),
                ..request(ICON)
            })
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-fetch-checksum-mismatch");
        assert!(setup.cache.kept.lock().is_empty());

        setup.hosts.body(ICON, b"PNG");
        let right = sum(b"PNG").to_uppercase();
        let asset = setup
            .assets
            .fetch(AssetRequest {
                sha256: Some(&right),
                ..request(ICON)
            })
            .await
            .unwrap();
        assert_eq!(asset.data, b"PNG");

        // a cached entry that does not have the checksum is replaced by a fresh download
        setup.hosts.body(ICON, b"NEW");
        let fresh = sum(b"NEW");
        let asset = setup
            .assets
            .fetch(AssetRequest {
                sha256: Some(&fresh),
                ..request(ICON)
            })
            .await
            .unwrap();
        assert_eq!(asset.data, b"NEW");
        assert_eq!(setup.hosts.asked().len(), 3);
    }

    #[tokio::test]
    async fn a_file_is_taken_out_of_an_archive_that_is_downloaded_once() {
        let setup = setup(true).await;
        let jar = "https://cdn.example.com/client.jar";
        setup.hosts.body(jar, b"a/stone.png=ROCK\npack.mcmeta={}\n");
        let stone = setup
            .assets
            .fetch(entry_of(jar, "a/stone.png"))
            .await
            .unwrap();
        assert_eq!(
            (stone.data.as_slice(), stone.content_type),
            (b"ROCK".as_slice(), "image/png")
        );
        let meta = setup
            .assets
            .fetch(entry_of(jar, "pack.mcmeta"))
            .await
            .unwrap();
        assert_eq!(meta.data, b"{}");
        assert_eq!(meta.content_type, "application/octet-stream");
        assert_eq!(setup.hosts.asked().len(), 1);
        // the file and the archive are kept; the file is served without the archive
        assert_eq!(setup.cache.kept.lock().len(), 3);
        let again = setup
            .assets
            .fetch(entry_of(jar, "a/stone.png"))
            .await
            .unwrap();
        assert_eq!(again, stone);

        assert_eq!(
            code_of(&setup, entry_of(jar, "missing.png")).await,
            "theme-fetch-entry-not-found"
        );
        let plain = "https://cdn.example.com/plain.zip";
        setup.hosts.body(plain, b"no archive");
        assert_eq!(
            code_of(&setup, entry_of(plain, "a.png")).await,
            "theme-fetch-not-archive"
        );
    }

    #[tokio::test]
    async fn the_checksum_of_an_archive_is_checked_before_a_file_is_taken() {
        let setup = setup(true).await;
        let jar = "https://cdn.example.com/client.jar";
        let archive = b"a.png=ROCK\n";
        setup.hosts.body(jar, archive);
        let wrong = "0".repeat(64);
        let error = setup
            .assets
            .fetch(AssetRequest {
                sha256: Some(&wrong),
                ..entry_of(jar, "a.png")
            })
            .await
            .unwrap_err();
        assert_eq!(error.code(), "theme-fetch-checksum-mismatch");
        assert!(setup.cache.kept.lock().is_empty());
        setup.hosts.body(jar, archive);
        let right = sum(archive);
        let stone = setup
            .assets
            .fetch(AssetRequest {
                sha256: Some(&right),
                ..entry_of(jar, "a.png")
            })
            .await
            .unwrap();
        assert_eq!(stone.data, b"ROCK");
    }

    #[tokio::test]
    async fn an_entry_that_is_not_a_plain_path_is_refused_before_any_request() {
        let setup = setup(true).await;
        let jar = "https://cdn.example.com/client.jar";
        for entry in [
            "",
            "../a.png",
            "/a.png",
            "a/../b.png",
            "a\\b.png",
            "C:/a.png",
            "a/",
        ] {
            assert_eq!(
                code_of(&setup, entry_of(jar, entry)).await,
                "theme-fetch-entry-invalid",
                "{entry:?}"
            );
        }
        assert_eq!(setup.hosts.asked(), Vec::<String>::new());
        assert!(setup.cache.kept.lock().is_empty());
    }

    #[tokio::test]
    async fn a_file_that_is_no_plain_file_in_the_archive_is_refused() {
        let setup = setup(true).await;
        let jar = "https://cdn.example.com/client.jar";
        setup.hosts.body(jar, b"a.png=ROCK\n");
        let assets = ThemeAssets::new(
            setup.fixture.service.clone(),
            setup.hosts.clone(),
            Arc::new(Unsafe),
            setup.cache.clone(),
        );
        let error = assets.fetch(entry_of(jar, "a.png")).await.unwrap_err();
        assert_eq!(error.code(), "theme-fetch-entry-invalid");
    }

    #[tokio::test]
    async fn the_code_going_off_during_a_download_stops_the_answer_and_the_cache() {
        let setup = setup(true).await;
        setup.hosts.body(ICON, b"PNG");
        *setup.hosts.gate.lock() = true;
        let fetching = {
            let assets = setup.assets.clone();
            tokio::spawn(async move { assets.fetch(request(ICON)).await })
        };
        setup.hosts.entered.arrived().await;
        setup
            .fixture
            .service
            .set_code("night-blue", false, None)
            .await
            .unwrap();
        setup.hosts.release.add_permits(1);
        let error = fetching.await.unwrap().unwrap_err();
        assert_eq!(error.code(), "theme-fetch-forbidden");
        assert!(setup.cache.kept.lock().is_empty());
    }

    #[tokio::test]
    async fn an_update_during_a_download_stops_the_answer_too() {
        let setup = setup(true).await;
        setup.hosts.body(ICON, b"PNG");
        *setup.hosts.gate.lock() = true;
        let fetching = {
            let assets = setup.assets.clone();
            tokio::spawn(async move { assets.fetch(request(ICON)).await })
        };
        setup.hosts.entered.arrived().await;
        // the person's update keeps the code on but replaces the theme
        setup
            .fixture
            .service
            .install(code_package("1.0.1", NETWORK), true, true)
            .await
            .unwrap();
        setup.hosts.release.add_permits(1);
        let error = fetching.await.unwrap().unwrap_err();
        assert_eq!(error.code(), "theme-fetch-forbidden");
        assert!(setup.cache.kept.lock().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_that_never_answers_ends_with_a_timeout() {
        let setup = setup(true).await;
        *setup.hosts.silent.lock() = true;
        let error = setup.assets.fetch(request(ICON)).await.unwrap_err();
        assert_eq!(error.code(), "theme-fetch-timeout");
        assert_eq!(setup.hosts.asked().len(), 1);
    }

    #[tokio::test]
    async fn a_cache_that_cannot_write_does_not_fail_the_request() {
        let setup = setup(true).await;
        *setup.cache.fail_put.lock() = true;
        setup.hosts.body(ICON, b"PNG");
        let asset = setup.assets.fetch(request(ICON)).await.unwrap();
        assert_eq!(asset.data, b"PNG");
        assert!(setup.cache.kept.lock().is_empty());
    }

    #[tokio::test]
    async fn the_cache_of_one_theme_is_not_read_for_another() {
        let setup = setup(true).await;
        let manifest = MANIFEST
            .replace("night-blue", "other-theme")
            .replace(r#""network": []"#, NETWORK);
        let package = archive(&[
            file("manifest.json", &manifest),
            file("theme.css", "a { color: red }"),
            file("main.js", "run()"),
        ]);
        setup
            .fixture
            .service
            .install(package, true, true)
            .await
            .unwrap();
        setup
            .fixture
            .service
            .set_code(
                "other-theme",
                true,
                Some(nullradix_domain::Version::INITIAL),
            )
            .await
            .unwrap();
        let from = |theme| AssetRequest {
            theme,
            ..request(ICON)
        };
        for theme in ["night-blue", "other-theme"] {
            setup.hosts.body(ICON, b"PNG");
            setup.assets.fetch(from(theme)).await.unwrap();
        }
        // the same address, two themes: two downloads and two entries
        assert_eq!(setup.hosts.asked().len(), 2);
        assert_eq!(setup.cache.kept.lock().len(), 2);
        for theme in ["night-blue", "other-theme"] {
            setup.assets.fetch(from(theme)).await.unwrap();
        }
        assert_eq!(setup.hosts.asked().len(), 2);
    }

    #[tokio::test]
    async fn parallel_requests_for_files_of_one_archive_download_it_once() {
        let setup = setup(true).await;
        let jar = "https://cdn.example.com/client.jar";
        setup.hosts.body(jar, b"a.png=ROCK\nb.png=SAND\n");
        *setup.hosts.gate.lock() = true;
        let fetching: Vec<_> = ["a.png", "b.png", "a.png"]
            .into_iter()
            .map(|entry| {
                let assets = setup.assets.clone();
                tokio::spawn(async move { assets.fetch(entry_of(jar, entry)).await })
            })
            .collect();
        setup.hosts.entered.arrived().await;
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert_eq!(setup.hosts.asked(), [jar]);
        setup.hosts.release.add_permits(1);
        let mut data = Vec::new();
        for task in fetching {
            data.push(task.await.unwrap().unwrap().data);
        }
        assert_eq!(data, [b"ROCK".to_vec(), b"SAND".to_vec(), b"ROCK".to_vec()]);
        assert_eq!(setup.hosts.asked(), [jar]);
        // nothing is left of the waiting lists
        assert!(setup.assets.flights.is_empty());
    }

    #[tokio::test]
    async fn a_failed_download_lets_the_next_request_try_again() {
        let setup = setup(true).await;
        setup.hosts.serve(ICON, vec![Err(FetchError::Unreachable)]);
        assert_eq!(
            code_of(&setup, request(ICON)).await,
            "theme-fetch-unreachable"
        );
        setup.hosts.body(ICON, b"PNG");
        assert_eq!(
            setup.assets.fetch(request(ICON)).await.unwrap().data,
            b"PNG"
        );
        assert!(setup.assets.flights.is_empty());
    }

    #[tokio::test]
    async fn no_more_than_four_downloads_run_at_the_same_time() {
        let setup = setup(true).await;
        let urls: Vec<String> = (0..=MAX_PARALLEL_DOWNLOADS)
            .map(|n| format!("https://cdn.example.com/{n}.png"))
            .collect();
        for url in &urls {
            setup.hosts.body(url, b"PNG");
        }
        *setup.hosts.gate.lock() = true;
        let fetching: Vec<_> = urls
            .iter()
            .map(|url| {
                let (assets, url) = (setup.assets.clone(), url.clone());
                tokio::spawn(async move { assets.fetch(request(&url)).await })
            })
            .collect();
        for _ in 0..MAX_PARALLEL_DOWNLOADS {
            setup.hosts.entered.arrived().await;
        }
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        // the fifth waits for a free place
        assert_eq!(setup.hosts.asked().len(), MAX_PARALLEL_DOWNLOADS);
        setup.hosts.release.add_permits(1);
        setup.hosts.entered.arrived().await;
        assert_eq!(setup.hosts.asked().len(), MAX_PARALLEL_DOWNLOADS + 1);
        setup.hosts.release.add_permits(MAX_PARALLEL_DOWNLOADS);
        for task in fetching {
            assert!(task.await.unwrap().is_ok());
        }
        assert_eq!(
            setup.hosts.peak.load(Ordering::SeqCst),
            MAX_PARALLEL_DOWNLOADS
        );
    }

    #[tokio::test]
    async fn after_a_failed_download_the_next_waiter_downloads_alone() {
        let setup = setup(true).await;
        setup.hosts.serve(
            ICON,
            vec![Err(FetchError::Unreachable), Ok(Hop::Body(b"PNG".to_vec()))],
        );
        *setup.hosts.gate.lock() = true;
        let spawn_fetch = || {
            let assets = setup.assets.clone();
            tokio::spawn(async move { assets.fetch(request(ICON)).await })
        };
        let first = spawn_fetch();
        setup.hosts.entered.arrived().await;
        let second = spawn_fetch();
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        setup.hosts.release.add_permits(1);
        assert_eq!(
            first.await.unwrap().unwrap_err().code(),
            "theme-fetch-unreachable"
        );
        // the second request downloads now; a third arrives while it runs
        setup.hosts.entered.arrived().await;
        let third = spawn_fetch();
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert_eq!(setup.hosts.asked().len(), 2);
        setup.hosts.release.add_permits(1);
        assert_eq!(second.await.unwrap().unwrap().data, b"PNG");
        assert_eq!(third.await.unwrap().unwrap().data, b"PNG");
        assert_eq!(setup.hosts.asked().len(), 2);
        assert_eq!(setup.hosts.peak.load(Ordering::SeqCst), 1);
        assert!(setup.assets.flights.is_empty());
    }

    #[tokio::test]
    async fn no_more_than_four_cached_archives_are_cut_open_at_the_same_time() {
        let setup = setup(true).await;
        let jar = "https://cdn.example.com/client.jar";
        let names: Vec<String> = (0..=MAX_PARALLEL_ARCHIVES)
            .map(|n| format!("{n}.png"))
            .collect();
        let archive = format!("{}=DATA\n", names.join("=DATA\n"));
        setup.cache.kept.lock().insert(
            ("night-blue".to_owned(), archive_key(jar)),
            archive.into_bytes(),
        );
        let gated = Arc::new(Gated::default());
        let assets = Arc::new(ThemeAssets::new(
            setup.fixture.service.clone(),
            setup.hosts.clone(),
            gated.clone(),
            setup.cache.clone(),
        ));
        let fetching: Vec<_> = names
            .iter()
            .map(|name| {
                let (assets, name) = (assets.clone(), name.clone());
                tokio::spawn(async move { assets.fetch(entry_of(jar, &name)).await })
            })
            .collect();
        for _ in 0..MAX_PARALLEL_ARCHIVES {
            gated.entered.arrived().await;
        }
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        // the fifth waits for a free place
        assert_eq!(gated.inflight.load(Ordering::SeqCst), MAX_PARALLEL_ARCHIVES);
        gated.release.add_permits(MAX_PARALLEL_ARCHIVES + 1);
        for task in fetching {
            assert_eq!(task.await.unwrap().unwrap().data, b"DATA");
        }
        assert_eq!(gated.peak.load(Ordering::SeqCst), MAX_PARALLEL_ARCHIVES);
        // the archive came from the cache every time
        assert_eq!(setup.hosts.asked(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn no_more_than_four_cached_archives_are_read_at_the_same_time() {
        let setup = setup(true).await;
        let cache = Arc::new(SlowReads::default());
        let jars: Vec<String> = (0..=MAX_PARALLEL_ARCHIVES)
            .map(|n| format!("https://cdn.example.com/{n}.jar"))
            .collect();
        for jar in &jars {
            let key = archive_key(jar);
            cache.slow.lock().push(key.clone());
            cache
                .memory
                .kept
                .lock()
                .insert(("night-blue".to_owned(), key), b"a.png=DATA\n".to_vec());
        }
        let assets = Arc::new(ThemeAssets::new(
            setup.fixture.service.clone(),
            setup.hosts.clone(),
            Arc::new(Lines),
            cache.clone(),
        ));
        let fetching: Vec<_> = jars
            .iter()
            .map(|jar| {
                let (assets, jar) = (assets.clone(), jar.clone());
                tokio::spawn(async move { assets.fetch(entry_of(&jar, "a.png")).await })
            })
            .collect();
        for _ in 0..MAX_PARALLEL_ARCHIVES {
            cache.entered.arrived().await;
        }
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        // the fifth archive waits for a free place before it is read
        assert_eq!(cache.inflight.load(Ordering::SeqCst), MAX_PARALLEL_ARCHIVES);
        cache.release.add_permits(MAX_PARALLEL_ARCHIVES + 1);
        for task in fetching {
            assert_eq!(task.await.unwrap().unwrap().data, b"DATA");
        }
        assert_eq!(cache.peak.load(Ordering::SeqCst), MAX_PARALLEL_ARCHIVES);
    }

    #[tokio::test]
    async fn a_slow_archive_with_many_waiters_does_not_hold_up_a_cached_one() {
        let setup = setup(true).await;
        let slow = "https://cdn.example.com/slow.jar";
        let cached = "https://cdn.example.com/cached.jar";
        setup
            .hosts
            .body(slow, b"a.png=1\nb.png=2\nc.png=3\nd.png=4\n");
        setup.cache.kept.lock().insert(
            ("night-blue".to_owned(), archive_key(cached)),
            b"y.png=SKY\n".to_vec(),
        );
        *setup.hosts.gate.lock() = true;
        let waiting: Vec<_> = ["a.png", "b.png", "c.png", "d.png"]
            .into_iter()
            .map(|name| {
                let assets = setup.assets.clone();
                tokio::spawn(async move { assets.fetch(entry_of(slow, name)).await })
            })
            .collect();
        // one request downloads, the other three wait for it
        setup.hosts.entered.arrived().await;
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        let other = {
            let assets = setup.assets.clone();
            tokio::spawn(async move { assets.fetch(entry_of(cached, "y.png")).await })
        };
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert!(
            other.is_finished(),
            "the cached archive waits for the slow one"
        );
        assert_eq!(other.await.unwrap().unwrap().data, b"SKY");
        setup.hosts.release.add_permits(1);
        for (task, expected) in waiting.into_iter().zip(["1", "2", "3", "4"]) {
            assert_eq!(task.await.unwrap().unwrap().data, expected.as_bytes());
        }
        assert_eq!(setup.hosts.asked(), [slow]);
        assert!(setup.assets.flights.is_empty());
    }

    #[tokio::test]
    async fn cold_reads_of_files_of_one_cached_archive_run_side_by_side() {
        let setup = setup(true).await;
        let jar = "https://cdn.example.com/client.jar";
        let cache = Arc::new(SlowReads::default());
        let key = archive_key(jar);
        cache.slow.lock().push(key.clone());
        cache.memory.kept.lock().insert(
            ("night-blue".to_owned(), key),
            b"a.png=1\nb.png=2\nc.png=3\n".to_vec(),
        );
        let assets = Arc::new(ThemeAssets::new(
            setup.fixture.service.clone(),
            setup.hosts.clone(),
            Arc::new(Lines),
            cache.clone(),
        ));
        let fetching: Vec<_> = ["a.png", "b.png", "c.png"]
            .into_iter()
            .map(|name| {
                let assets = assets.clone();
                tokio::spawn(async move { assets.fetch(entry_of(jar, name)).await })
            })
            .collect();
        // all three reads are under way at once, none waits for another
        for _ in 0..3 {
            cache.entered.arrived().await;
        }
        assert_eq!(cache.inflight.load(Ordering::SeqCst), 3);
        cache.release.add_permits(3);
        for (task, expected) in fetching.into_iter().zip(["1", "2", "3"]) {
            assert_eq!(task.await.unwrap().unwrap().data, expected.as_bytes());
        }
        assert_eq!(setup.hosts.asked(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn the_next_request_reads_the_archive_while_the_first_cuts_its_file() {
        let setup = setup(true).await;
        let jar = "https://cdn.example.com/client.jar";
        setup.hosts.body(jar, b"a.png=1\nb.png=2\n");
        let gated = Arc::new(Gated::default());
        let assets = Arc::new(ThemeAssets::new(
            setup.fixture.service.clone(),
            setup.hosts.clone(),
            gated.clone(),
            setup.cache.clone(),
        ));
        *setup.hosts.gate.lock() = true;
        let fetch = |name: &'static str| {
            let assets = assets.clone();
            tokio::spawn(async move { assets.fetch(entry_of(jar, name)).await })
        };
        let first = fetch("a.png");
        setup.hosts.entered.arrived().await;
        // the second waits for the download of the first
        let second = fetch("b.png");
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        setup.hosts.release.add_permits(1);
        let fetching = [first, second];
        // one downloads and cuts; the other reads the cached archive and cuts too
        for _ in 0..2 {
            gated.entered.arrived().await;
        }
        assert_eq!(gated.inflight.load(Ordering::SeqCst), 2);
        gated.release.add_permits(2);
        for (task, expected) in fetching.into_iter().zip(["1", "2"]) {
            assert_eq!(task.await.unwrap().unwrap().data, expected.as_bytes());
        }
        assert_eq!(setup.hosts.asked(), [jar]);
        assert!(assets.flights.is_empty());
    }

    #[tokio::test]
    async fn a_request_that_waited_for_a_place_takes_the_file_cut_meanwhile() {
        let setup = setup(true).await;
        let jar = "https://cdn.example.com/client.jar";
        setup.hosts.body(jar, b"a.png=1\n");
        let busy: Vec<String> = (0..MAX_PARALLEL_ARCHIVES - 1)
            .map(|n| format!("https://cdn.example.com/{n}.jar"))
            .collect();
        for other in &busy {
            setup.cache.kept.lock().insert(
                ("night-blue".to_owned(), archive_key(other)),
                b"a.png=DATA\n".to_vec(),
            );
        }
        let gated = Arc::new(Gated::default());
        let assets = Arc::new(ThemeAssets::new(
            setup.fixture.service.clone(),
            setup.hosts.clone(),
            gated.clone(),
            setup.cache.clone(),
        ));
        let fetch = |url: String| {
            let assets = assets.clone();
            tokio::spawn(async move { assets.fetch(entry_of(&url, "a.png")).await })
        };
        *setup.hosts.gate.lock() = true;
        let first = fetch(jar.to_owned());
        setup.hosts.entered.arrived().await;
        // the second waits for the download of the first, without a place of its own
        let second = fetch(jar.to_owned());
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        let cutting: Vec<_> = busy.iter().cloned().map(fetch).collect();
        for _ in 0..busy.len() {
            gated.entered.arrived().await;
        }
        setup.hosts.release.add_permits(1);
        // the first cuts now and all places are taken, so the second waits for one
        gated.entered.arrived().await;
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        gated.release.add_permits(MAX_PARALLEL_ARCHIVES + 1);
        assert_eq!(first.await.unwrap().unwrap().data, b"1");
        assert_eq!(second.await.unwrap().unwrap().data, b"1");
        for task in cutting {
            assert_eq!(task.await.unwrap().unwrap().data, b"DATA");
        }
        // the file was cut once for both
        assert_eq!(gated.calls.load(Ordering::SeqCst), MAX_PARALLEL_ARCHIVES);
        assert_eq!(setup.hosts.asked(), [jar]);
    }

    #[tokio::test]
    async fn a_host_without_network_cannot_load_anything() {
        let setup = setup(true).await;
        let assets = ThemeAssets::new(
            setup.fixture.service.clone(),
            Arc::new(NoAssetNetwork),
            Arc::new(NoZipEntries),
            Arc::new(NoAssetCache),
        );
        let error = assets.fetch(request(ICON)).await.unwrap_err();
        assert_eq!(error.code(), "theme-fetch-unreachable");
        let error = assets.fetch(entry_of(ICON, "a.png")).await.unwrap_err();
        assert_eq!(error.code(), "theme-fetch-unreachable");
    }
}
