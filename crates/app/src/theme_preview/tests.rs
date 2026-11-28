use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nullradix_domain::{CatalogUrl, Reach, Version};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::*;
use crate::ErrorKind;
use crate::theme_catalog::tests::{FakeFetcher, SOURCE, World, world, world_with};

const SHEET: &str = "https://themes.example.com/ocean.png";
const PNG_HEAD: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
const JPEG_HEAD: [u8; 4] = [0xff, 0xd8, 0xff, 0xe0];

fn png(len: usize) -> Vec<u8> {
    let mut bytes = PNG_HEAD.to_vec();
    bytes.resize(len, 7);
    bytes
}

fn jpeg() -> Vec<u8> {
    let mut bytes = JPEG_HEAD.to_vec();
    bytes.extend_from_slice(b"JFIF");
    bytes
}

fn webp() -> Vec<u8> {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&[0x1a, 0, 0, 0]);
    bytes.extend_from_slice(b"WEBPVP8 ");
    bytes
}

/// A semaphore without permits at the start.
struct Permits(tokio::sync::Semaphore);

impl Default for Permits {
    fn default() -> Self {
        Self(tokio::sync::Semaphore::new(0))
    }
}

impl Permits {
    /// Waits for one arrival and fails the test if none comes.
    async fn arrived(&self) {
        let permit = tokio::time::timeout(Duration::from_secs(5), self.0.acquire())
            .await
            .expect("nothing reached the gate within 5 seconds");
        permit.unwrap().forget();
    }
}

/// Images by address, each request held at a gate until released.
#[derive(Default)]
struct Hosts {
    answers: Mutex<BTreeMap<String, Result<Vec<u8>, FetchError>>>,
    asked: Mutex<Vec<(String, u64)>>,
    gate: Mutex<bool>,
    entered: Permits,
    release: Permits,
    inflight: AtomicUsize,
    peak: AtomicUsize,
}

impl Hosts {
    fn serve(&self, url: &str, result: Result<Vec<u8>, FetchError>) {
        self.answers.lock().insert(url.to_owned(), result);
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
impl CatalogFetcher for Hosts {
    async fn fetch(&self, url: &CatalogUrl, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
        self.asked.lock().push((url.to_string(), max_bytes));
        let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let held = *self.gate.lock();
        if held {
            self.entered.0.add_permits(1);
            self.release.0.acquire().await.unwrap().forget();
        }
        self.inflight.fetch_sub(1, Ordering::SeqCst);
        self.answers
            .lock()
            .get(url.as_str())
            .cloned()
            .unwrap_or(Err(FetchError::Unreachable))
    }
}

/// Kept files in memory, handing the bytes back when a write fails.
#[derive(Default)]
struct MemoryPreviews {
    files: Mutex<BTreeMap<(String, String), Vec<u8>>>,
    puts: AtomicUsize,
    failing: Mutex<bool>,
    hold_puts: Mutex<bool>,
    put_entered: Permits,
    put_release: Permits,
}

impl MemoryPreviews {
    fn kept(&self) -> usize {
        self.files.lock().len()
    }
}

#[async_trait]
impl PreviewCache for MemoryPreviews {
    async fn get(&self, source: &SourceId, key: &str) -> Option<Vec<u8>> {
        self.files
            .lock()
            .get(&(source.to_string(), key.to_owned()))
            .cloned()
    }

    async fn put(&self, source: &SourceId, key: &str, bytes: Vec<u8>) -> Result<Stored, AppError> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        let held = *self.hold_puts.lock();
        if held {
            self.put_entered.0.add_permits(1);
            self.put_release.0.acquire().await.unwrap().forget();
        }
        if *self.failing.lock() {
            return Ok(Stored {
                bytes,
                error: Some(AppError::unavailable("files-failed", "disk full")),
            });
        }
        self.files
            .lock()
            .insert((source.to_string(), key.to_owned()), bytes.clone());
        Ok(Stored { bytes, error: None })
    }

    async fn clear(&self, source: &SourceId) -> Result<(), AppError> {
        self.files
            .lock()
            .retain(|(owner, _), _| owner != source.as_str());
        Ok(())
    }
}

/// The index fetcher with a gate: a read waits until it is released.
struct SlowIndex {
    inner: Arc<FakeFetcher>,
    arrivals: AtomicUsize,
    entered: Permits,
    release: Permits,
}

#[async_trait]
impl CatalogFetcher for SlowIndex {
    async fn fetch(&self, url: &CatalogUrl, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
        self.arrivals.fetch_add(1, Ordering::SeqCst);
        self.entered.0.add_permits(1);
        self.release.0.acquire().await.unwrap().forget();
        self.inner.fetch(url, max_bytes).await
    }
}

struct Setup {
    world: World,
    hosts: Arc<Hosts>,
    cache: Arc<MemoryPreviews>,
    previews: Arc<ThemePreviews>,
    source: String,
}

async fn setup() -> Setup {
    let world = world(Reach::Public);
    ready(world).await
}

async fn ready(world: World) -> Setup {
    let source = world.service.add_source(SOURCE).await.unwrap();
    let hosts = Arc::new(Hosts::default());
    let cache = Arc::new(MemoryPreviews::default());
    let previews = Arc::new(ThemePreviews::new(
        world.service.clone(),
        hosts.clone(),
        cache.clone(),
    ));
    Setup {
        world,
        hosts,
        cache,
        previews,
        source: source.id().to_string(),
    }
}

fn entry(id: &str, preview: Option<&str>, sum: char) -> Value {
    let mut entry = json!({
        "id": id, "name": id, "version": "1.0.0", "sdkVersion": "^2.0",
        "downloadUrl": format!("https://themes.example.com/{id}.nullradix"),
        "sha256": sum.to_string().repeat(64),
    });
    if let Some(preview) = preview {
        entry["preview"] = json!(preview);
    }
    entry
}

fn offer(setup: &Setup, entries: &[Value]) {
    setup
        .world
        .fetcher
        .answer(SOURCE, Ok(serde_json::to_vec(entries).unwrap()));
}

/// The way a page gets here: a browse has read the index.
async fn browsed(entries: &[Value]) -> Setup {
    let setup = setup().await;
    offer(&setup, entries);
    setup.world.service.browse().await.unwrap();
    setup
}

async fn failure(setup: &Setup, theme: &str) -> AppError {
    setup.previews.get(&setup.source, theme).await.unwrap_err()
}

#[tokio::test]
async fn a_preview_is_fetched_once_and_then_served_from_the_cache() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    setup.hosts.serve(SHEET, Ok(png(100)));
    let first = setup.previews.get(&setup.source, "ocean").await.unwrap();
    assert_eq!(first.data, png(100));
    assert_eq!(first.content_type, "image/png");
    let again = setup.previews.get(&setup.source, "ocean").await.unwrap();
    assert_eq!(again, first);
    assert_eq!(
        setup.hosts.asked.lock().clone(),
        [(SHEET.to_owned(), MAX_PREVIEW_BYTES)]
    );
    assert_eq!(setup.cache.kept(), 1);
    // the index was read by the browse and not for the previews
    assert_eq!(setup.world.fetcher.asked.lock().len(), 1);
    assert!(setup.previews.flights.is_empty());
}

#[tokio::test]
async fn a_preview_changes_with_its_address_and_with_the_checksum_of_the_entry() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    setup.hosts.serve(SHEET, Ok(png(100)));
    setup.previews.get(&setup.source, "ocean").await.unwrap();

    // a renamed entry with the same address and checksum has the same image
    let mut renamed = entry("ocean", Some(SHEET), 'a');
    renamed["name"] = json!("Ocean II");
    offer(&setup, &[renamed]);
    setup.world.service.browse().await.unwrap();
    setup.previews.get(&setup.source, "ocean").await.unwrap();
    assert_eq!(setup.hosts.asked().len(), 1);

    // a new package checksum brings a new image from the same address
    offer(&setup, &[entry("ocean", Some(SHEET), 'b')]);
    setup.world.service.browse().await.unwrap();
    setup.hosts.serve(SHEET, Ok(png(200)));
    let renewed = setup.previews.get(&setup.source, "ocean").await.unwrap();
    assert_eq!(renewed.data, png(200));
    assert_eq!(setup.hosts.asked().len(), 2);

    // so does a new address
    let other = "https://themes.example.com/ocean-2.png";
    offer(&setup, &[entry("ocean", Some(other), 'b')]);
    setup.world.service.browse().await.unwrap();
    setup.hosts.serve(other, Ok(jpeg()));
    let moved = setup.previews.get(&setup.source, "ocean").await.unwrap();
    assert_eq!((moved.data, moved.content_type), (jpeg(), "image/jpeg"));
    assert_eq!(setup.hosts.asked().last().map(String::as_str), Some(other));
    assert_ne!(renewed.version, moved.version);
}

#[tokio::test]
async fn only_png_jpeg_and_webp_are_served_and_by_what_they_are() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    for (bytes, kind) in [
        (png(50), "image/png"),
        (jpeg(), "image/jpeg"),
        (webp(), "image/webp"),
    ] {
        setup.cache.files.lock().clear();
        setup.hosts.serve(SHEET, Ok(bytes.clone()));
        let served = setup.previews.get(&setup.source, "ocean").await.unwrap();
        assert_eq!((served.data, served.content_type), (bytes, kind));
    }
    setup.cache.files.lock().clear();
    setup.cache.puts.store(0, Ordering::SeqCst);
    let refused: [&[u8]; 6] = [
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>",
        b"<!doctype html><script>alert(1)</script>",
        b"GIF89a-animated",
        b"RIFF\0\0\0\0WAVEfmt ",
        b"",
        b"{\"preview\":true}",
    ];
    for bytes in refused {
        setup.hosts.serve(SHEET, Ok(bytes.to_vec()));
        let error = failure(&setup, "ocean").await;
        assert_eq!(
            (error.kind(), error.code()),
            (ErrorKind::Invalid, "catalog-preview-invalid"),
            "{bytes:?}"
        );
    }
    // Nothing but an image is kept.
    assert_eq!(setup.cache.puts.load(Ordering::SeqCst), 0);
    assert_eq!(setup.cache.kept(), 0);
}

#[tokio::test]
async fn a_preview_just_above_the_cap_is_refused_and_one_at_the_cap_is_served() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    let cap = usize::try_from(MAX_PREVIEW_BYTES).unwrap();
    setup.hosts.serve(SHEET, Ok(png(cap)));
    let served = setup.previews.get(&setup.source, "ocean").await.unwrap();
    assert_eq!(served.data.len(), cap);

    setup.cache.files.lock().clear();
    setup.hosts.serve(SHEET, Ok(png(cap + 1)));
    let error = failure(&setup, "ocean").await;
    assert_eq!(
        (error.kind(), error.code()),
        (ErrorKind::Invalid, "catalog-too-large")
    );
    // the adapter reports the same when it stops reading
    setup.hosts.serve(SHEET, Err(FetchError::TooLarge));
    assert_eq!(failure(&setup, "ocean").await.code(), "catalog-too-large");
    assert_eq!(setup.cache.kept(), 0);
}

#[tokio::test]
async fn a_host_that_fails_is_told_apart_and_asked_again_next_time() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    for (error, kind, code) in [
        (
            FetchError::Refused,
            ErrorKind::Invalid,
            "catalog-address-refused",
        ),
        (
            FetchError::Timeout,
            ErrorKind::Unavailable,
            "catalog-timeout",
        ),
        (
            FetchError::Unreachable,
            ErrorKind::Unavailable,
            "catalog-unreachable",
        ),
        (
            FetchError::Status(404),
            ErrorKind::Unavailable,
            "catalog-http-error",
        ),
    ] {
        setup.hosts.serve(SHEET, Err(error));
        let failed = failure(&setup, "ocean").await;
        assert_eq!((failed.kind(), failed.code()), (kind, code));
    }
    assert_eq!(setup.hosts.asked().len(), 4);
    setup.hosts.serve(SHEET, Ok(png(10)));
    assert!(setup.previews.get(&setup.source, "ocean").await.is_ok());
    assert!(setup.previews.flights.is_empty());
}

#[tokio::test]
async fn a_theme_without_a_preview_or_one_the_source_lacks_is_not_found() {
    let setup = browsed(&[entry("ocean", None, 'a')]).await;
    let bare = failure(&setup, "ocean").await;
    assert_eq!(
        (bare.kind(), bare.code()),
        (ErrorKind::NotFound, "catalog-preview-not-found")
    );
    let unknown = failure(&setup, "forest").await;
    assert_eq!(
        (unknown.kind(), unknown.code()),
        (ErrorKind::NotFound, "catalog-theme-not-found")
    );
    assert_eq!(setup.hosts.asked(), Vec::<String>::new());
}

#[tokio::test]
async fn an_entry_for_another_sdk_still_has_its_preview() {
    let mut other = entry("ocean", Some(SHEET), 'a');
    other["sdkVersion"] = json!("^9.0");
    let setup = browsed(&[other]).await;
    setup.hosts.serve(SHEET, Ok(png(10)));
    assert!(setup.previews.get(&setup.source, "ocean").await.is_ok());
}

#[tokio::test]
async fn a_bad_id_or_an_unknown_source_is_refused_before_anything_is_fetched() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    let asked = setup.world.fetcher.asked.lock().len();
    for (source, theme, kind, code) in [
        (
            "NOT-A-SOURCE",
            "ocean",
            ErrorKind::NotFound,
            "catalog-source-not-found",
        ),
        (
            "000000000000",
            "ocean",
            ErrorKind::NotFound,
            "catalog-source-not-found",
        ),
        (
            setup.source.as_str(),
            "Bad Id",
            ErrorKind::Invalid,
            "theme-id-invalid",
        ),
        (
            setup.source.as_str(),
            "../ocean",
            ErrorKind::Invalid,
            "theme-id-invalid",
        ),
    ] {
        let error = setup.previews.get(source, theme).await.unwrap_err();
        assert_eq!(
            (error.kind(), error.code()),
            (kind, code),
            "{source} {theme}"
        );
    }
    assert_eq!(setup.hosts.asked(), Vec::<String>::new());
    assert_eq!(setup.world.fetcher.asked.lock().len(), asked);
}

#[tokio::test]
async fn an_entry_with_a_preview_that_is_not_public_https_is_skipped() {
    let setup = setup().await;
    let entries: Vec<Value> = [
        "http://themes.example.com/a.png",
        "https://10.0.0.5/a.png",
        "https://localhost/a.png",
        "https://u:p@themes.example.com/a.png",
        "https://themes.example.com:8443/a.png",
        "file:///etc/passwd",
    ]
    .iter()
    .enumerate()
    .map(|(number, preview)| entry(&format!("theme-{number}"), Some(*preview), 'a'))
    .chain([entry("fine", Some(SHEET), 'a')])
    .collect();
    offer(&setup, &entries);
    let catalog = setup.world.service.browse().await.unwrap();
    assert_eq!(catalog.themes.len(), 1);
    let status = &catalog.sources[0];
    assert_eq!(status.skipped.len(), 6);
    assert!(
        status
            .skipped
            .iter()
            .all(|skipped| skipped.reason.starts_with("preview: "))
    );
    assert_eq!(
        failure(&setup, "theme-0").await.code(),
        "catalog-theme-not-found"
    );
}

#[tokio::test]
async fn the_index_is_read_once_when_previews_ask_before_any_was_read() {
    let held = Arc::new(Mutex::new(None::<Arc<SlowIndex>>));
    let keep = held.clone();
    let world = world_with(Reach::Public, move |fetcher| {
        let slow = Arc::new(SlowIndex {
            inner: fetcher,
            arrivals: AtomicUsize::new(0),
            entered: Permits::default(),
            release: Permits::default(),
        });
        *keep.lock() = Some(slow.clone());
        slow
    });
    let setup = ready(world).await;
    let reads = held.lock().clone().unwrap();
    offer(
        &setup,
        &[entry("ocean", Some(SHEET), 'a'), entry("forest", None, 'b')],
    );
    setup.hosts.serve(SHEET, Ok(png(10)));
    let asking: Vec<_> = ["ocean", "ocean", "forest"]
        .into_iter()
        .map(|theme| {
            let (previews, source) = (setup.previews.clone(), setup.source.clone());
            tokio::spawn(async move { previews.get(&source, theme).await })
        })
        .collect();
    reads.entered.arrived().await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    // The others wait for the read in flight.
    assert_eq!(reads.arrivals.load(Ordering::SeqCst), 1);
    reads.release.0.add_permits(1);
    let mut codes = Vec::new();
    for task in asking {
        codes.push(
            task.await
                .unwrap()
                .map_err(|error| error.code())
                .map(|p| p.data),
        );
    }
    assert_eq!(
        codes,
        [Ok(png(10)), Ok(png(10)), Err("catalog-preview-not-found")]
    );
    assert_eq!(setup.world.fetcher.asked.lock().len(), 1);
    assert!(setup.world.service.reads_are_idle());
}

#[tokio::test]
async fn a_failing_index_is_reported_as_the_source_failing() {
    let setup = setup().await;
    setup.world.fetcher.answer(SOURCE, Err(FetchError::Timeout));
    let error = failure(&setup, "ocean").await;
    assert_eq!(
        (error.kind(), error.code()),
        (ErrorKind::Unavailable, "catalog-timeout")
    );
    setup.world.fetcher.answer(SOURCE, Ok(b"{}".to_vec()));
    assert_eq!(
        failure(&setup, "ocean").await.code(),
        "catalog-index-invalid"
    );
    assert!(setup.world.service.reads_are_idle());
}

#[tokio::test]
async fn one_download_serves_everyone_who_asks_for_the_same_preview() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    setup.hosts.serve(SHEET, Ok(png(10)));
    *setup.hosts.gate.lock() = true;
    let asking: Vec<_> = (0..3)
        .map(|_| {
            let (previews, source) = (setup.previews.clone(), setup.source.clone());
            tokio::spawn(async move { previews.get(&source, "ocean").await })
        })
        .collect();
    setup.hosts.entered.arrived().await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(setup.hosts.asked().len(), 1);
    setup.hosts.release.0.add_permits(1);
    for task in asking {
        assert_eq!(task.await.unwrap().unwrap().data, png(10));
    }
    assert_eq!(setup.hosts.asked().len(), 1);
    assert_eq!(setup.cache.puts.load(Ordering::SeqCst), 1);
    assert!(setup.previews.flights.is_empty());
}

#[tokio::test]
async fn no_more_than_four_previews_download_at_the_same_time() {
    let ids: Vec<String> = (0..=MAX_PARALLEL_PREVIEWS)
        .map(|number| format!("theme-{number}"))
        .collect();
    let entries: Vec<Value> = ids
        .iter()
        .map(|id| {
            entry(
                id,
                Some(&format!("https://themes.example.com/{id}.png")),
                'a',
            )
        })
        .collect();
    let setup = browsed(&entries).await;
    for id in &ids {
        setup
            .hosts
            .serve(&format!("https://themes.example.com/{id}.png"), Ok(png(10)));
    }
    *setup.hosts.gate.lock() = true;
    let asking: Vec<_> = ids
        .iter()
        .map(|id| {
            let (previews, source, id) = (setup.previews.clone(), setup.source.clone(), id.clone());
            tokio::spawn(async move { previews.get(&source, &id).await })
        })
        .collect();
    for _ in 0..MAX_PARALLEL_PREVIEWS {
        setup.hosts.entered.arrived().await;
    }
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    // the fifth waits for a free place
    assert_eq!(setup.hosts.asked().len(), MAX_PARALLEL_PREVIEWS);
    setup.hosts.release.0.add_permits(MAX_PARALLEL_PREVIEWS + 1);
    for task in asking {
        assert!(task.await.unwrap().is_ok());
    }
    assert_eq!(
        setup.hosts.peak.load(Ordering::SeqCst),
        MAX_PARALLEL_PREVIEWS
    );
}

#[tokio::test]
async fn a_source_removed_during_the_download_keeps_nothing() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    setup.hosts.serve(SHEET, Ok(png(10)));
    *setup.hosts.gate.lock() = true;
    let asking = {
        let (previews, source) = (setup.previews.clone(), setup.source.clone());
        tokio::spawn(async move { previews.get(&source, "ocean").await })
    };
    setup.hosts.entered.arrived().await;
    setup
        .world
        .service
        .remove_source(&setup.source, Version::from_stored(1))
        .await
        .unwrap();
    setup.hosts.release.0.add_permits(1);
    let error = asking.await.unwrap().unwrap_err();
    assert_eq!(error.code(), "catalog-source-not-found");
    assert_eq!(setup.cache.puts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_removed_source_offers_no_previews_even_when_it_is_added_again() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    setup.hosts.serve(SHEET, Ok(png(10)));
    setup.previews.get(&setup.source, "ocean").await.unwrap();
    setup
        .world
        .service
        .remove_source(&setup.source, Version::from_stored(1))
        .await
        .unwrap();
    setup.previews.forget(&setup.source).await;
    assert_eq!(setup.cache.kept(), 0);
    assert_eq!(
        failure(&setup, "ocean").await.code(),
        "catalog-source-not-found"
    );
    // the index is read anew, not taken from what was remembered
    setup.world.service.add_source(SOURCE).await.unwrap();
    setup
        .world
        .fetcher
        .answer(SOURCE, Err(FetchError::Unreachable));
    assert_eq!(failure(&setup, "ocean").await.code(), "catalog-unreachable");
}

#[tokio::test]
async fn forgetting_a_source_empties_only_its_files() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    setup.hosts.serve(SHEET, Ok(png(10)));
    setup.previews.get(&setup.source, "ocean").await.unwrap();
    let other = SourceId::parse("abcdefabcdef").unwrap();
    setup.cache.put(&other, "k", png(10)).await.unwrap();
    setup.previews.forget("not-a-source").await;
    assert_eq!(setup.cache.kept(), 2);
    setup.previews.forget(&setup.source).await;
    assert_eq!(setup.cache.kept(), 1);
    assert!(setup.cache.get(&other, "k").await.is_some());
}

#[tokio::test]
async fn a_kept_file_that_is_no_image_is_fetched_anew() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    setup.hosts.serve(SHEET, Ok(png(10)));
    setup.previews.get(&setup.source, "ocean").await.unwrap();
    let key = setup.cache.files.lock().keys().next().cloned().unwrap();
    setup
        .cache
        .files
        .lock()
        .insert(key.clone(), b"<svg onload=alert(1)/>".to_vec());
    let served = setup.previews.get(&setup.source, "ocean").await.unwrap();
    assert_eq!(served.data, png(10));
    assert_eq!(setup.hosts.asked().len(), 2);
    assert_eq!(setup.cache.files.lock().get(&key), Some(&png(10)));
}

#[tokio::test]
async fn a_cache_that_cannot_write_still_serves_the_image() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    setup.hosts.serve(SHEET, Ok(png(10)));
    *setup.cache.failing.lock() = true;
    let served = setup.previews.get(&setup.source, "ocean").await.unwrap();
    assert_eq!(served.data, png(10));
    assert_eq!(setup.cache.kept(), 0);
}

#[tokio::test]
async fn a_source_added_again_during_the_download_keeps_nothing_and_reads_no_index() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    setup.hosts.serve(SHEET, Ok(png(10)));
    *setup.hosts.gate.lock() = true;
    let asking = {
        let (previews, source) = (setup.previews.clone(), setup.source.clone());
        tokio::spawn(async move { previews.get(&source, "ocean").await })
    };
    setup.hosts.entered.arrived().await;
    let service = &setup.world.service;
    service
        .remove_source(&setup.source, Version::from_stored(1))
        .await
        .unwrap();
    service.add_source(SOURCE).await.unwrap();
    setup.hosts.release.0.add_permits(1);
    let error = asking.await.unwrap().unwrap_err();
    assert_eq!(
        (error.kind(), error.code()),
        (ErrorKind::NotFound, "catalog-theme-not-found")
    );
    assert_eq!(setup.cache.puts.load(Ordering::SeqCst), 0);
    // only the browse read the index, the re-check under the flight did not
    assert_eq!(setup.world.fetcher.asked.lock().len(), 1);
    assert!(service.reads_are_idle());
    assert!(setup.previews.flights.is_empty());
}

#[tokio::test]
async fn a_source_removed_while_its_file_is_written_leaves_no_file() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    setup.hosts.serve(SHEET, Ok(png(10)));
    *setup.cache.hold_puts.lock() = true;
    let asking = {
        let (previews, source) = (setup.previews.clone(), setup.source.clone());
        tokio::spawn(async move { previews.get(&source, "ocean").await })
    };
    setup.cache.put_entered.arrived().await;
    // The remove handler runs before the write ends.
    setup
        .world
        .service
        .remove_source(&setup.source, Version::from_stored(1))
        .await
        .unwrap();
    setup.previews.forget(&setup.source).await;
    setup.cache.put_release.0.add_permits(1);
    let error = asking.await.unwrap().unwrap_err();
    assert_eq!(error.code(), "catalog-source-not-found");
    assert_eq!(setup.cache.kept(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_download_that_never_ends_fails_as_a_timeout_and_frees_its_place() {
    let setup = browsed(&[entry("ocean", Some(SHEET), 'a')]).await;
    setup.hosts.serve(SHEET, Ok(png(10)));
    *setup.hosts.gate.lock() = true;
    let asking = {
        let (previews, source) = (setup.previews.clone(), setup.source.clone());
        tokio::spawn(async move { previews.get(&source, "ocean").await })
    };
    // A paused clock makes the wait for the budget free.
    let outcome = tokio::time::timeout(Duration::from_secs(120), asking)
        .await
        .expect("the download was not cut off")
        .unwrap();
    let error = outcome.unwrap_err();
    assert_eq!(
        (error.kind(), error.code()),
        (ErrorKind::Unavailable, "catalog-timeout")
    );
    assert_eq!(
        setup.previews.downloads.available_permits(),
        MAX_PARALLEL_PREVIEWS
    );
    assert!(setup.previews.flights.is_empty());
}

/// A world whose index reads wait for a release.
async fn slow_index() -> (Setup, Arc<SlowIndex>) {
    let held = Arc::new(Mutex::new(None::<Arc<SlowIndex>>));
    let keep = held.clone();
    let world = world_with(Reach::Public, move |fetcher| {
        let slow = Arc::new(SlowIndex {
            inner: fetcher,
            arrivals: AtomicUsize::new(0),
            entered: Permits::default(),
            release: Permits::default(),
        });
        *keep.lock() = Some(slow.clone());
        slow
    });
    let setup = ready(world).await;
    let reads = held.lock().clone().unwrap();
    offer(&setup, &[entry("ocean", Some(SHEET), 'a')]);
    (setup, reads)
}

async fn offered_code(setup: &Setup) -> Option<&'static str> {
    let source = SourceId::parse(&setup.source).unwrap();
    let theme = ThemeId::parse("ocean").unwrap();
    setup
        .world
        .service
        .offered_preview(&source, &theme)
        .await
        .err()
        .map(|error| error.code())
}

#[tokio::test]
async fn an_index_read_that_ends_after_a_removal_and_an_add_offers_nothing() {
    let (setup, reads) = slow_index().await;
    let service = setup.world.service.clone();
    let browsing = tokio::spawn(async move { service.browse().await });
    reads.entered.arrived().await;
    let service = &setup.world.service;
    service
        .remove_source(&setup.source, Version::from_stored(1))
        .await
        .unwrap();
    service.add_source(SOURCE).await.unwrap();
    reads.release.0.add_permits(1);
    browsing.await.unwrap().unwrap();
    assert_eq!(offered_code(&setup).await, Some("catalog-theme-not-found"));
}

#[tokio::test]
async fn an_index_read_that_ends_after_a_removal_offers_nothing_when_the_source_returns() {
    let (setup, reads) = slow_index().await;
    let service = setup.world.service.clone();
    let browsing = tokio::spawn(async move { service.browse().await });
    reads.entered.arrived().await;
    setup
        .world
        .service
        .remove_source(&setup.source, Version::from_stored(1))
        .await
        .unwrap();
    reads.release.0.add_permits(1);
    browsing.await.unwrap().unwrap();
    setup.world.service.add_source(SOURCE).await.unwrap();
    assert_eq!(offered_code(&setup).await, Some("catalog-theme-not-found"));
}
