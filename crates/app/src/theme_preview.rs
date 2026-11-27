//! The preview proxy: images of catalog entries are fetched here, never by the page.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{MAX_PREVIEW_BYTES, PreviewImage, SourceId, ThemeId};
use sha2::{Digest, Sha256};

use crate::flights::Flights;
use crate::theme_catalog::{CatalogFetcher, CatalogService, FetchError, fetch_failed};
use crate::theme_fetch::Stored;
use crate::{AppError, auth};

/// Previews fetched at the same time, all sources together.
pub const MAX_PARALLEL_PREVIEWS: usize = 4;

/// Bytes kept per source under a key; adapters keep the total below a budget.
#[async_trait]
pub trait PreviewCache: Send + Sync {
    /// A miss and an unreadable entry both give `None`.
    async fn get(&self, source: &SourceId, key: &str) -> Option<Vec<u8>>;
    async fn put(&self, source: &SourceId, key: &str, bytes: Vec<u8>) -> Result<Stored, AppError>;
    /// Drops everything kept for the source.
    async fn clear(&self, source: &SourceId) -> Result<(), AppError>;
}

/// A cache that keeps nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPreviewCache;

#[async_trait]
impl PreviewCache for NoPreviewCache {
    async fn get(&self, _source: &SourceId, _key: &str) -> Option<Vec<u8>> {
        None
    }

    async fn put(
        &self,
        _source: &SourceId,
        _key: &str,
        bytes: Vec<u8>,
    ) -> Result<Stored, AppError> {
        Ok(Stored { bytes, error: None })
    }

    async fn clear(&self, _source: &SourceId) -> Result<(), AppError> {
        Ok(())
    }
}

/// A preview image ready to be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub data: Vec<u8>,
    pub content_type: &'static str,
    /// What the entry calls this image; a page that asked for it by this version may keep it long.
    pub version: String,
}

fn too_large() -> AppError {
    AppError::invalid(
        "catalog-too-large",
        format!("the preview is larger than {MAX_PREVIEW_BYTES} bytes"),
    )
}

fn not_an_image() -> AppError {
    AppError::invalid(
        "catalog-preview-invalid",
        "the preview is not a PNG, JPEG or WebP image",
    )
}

fn cache_key(theme: &ThemeId, fingerprint: &str) -> String {
    auth::hex(&Sha256::digest(
        format!("{theme}\n{fingerprint}").as_bytes(),
    ))
}

pub struct ThemePreviews {
    catalog: Arc<CatalogService>,
    fetcher: Arc<dyn CatalogFetcher>,
    cache: Arc<dyn PreviewCache>,
    flights: Flights<(SourceId, ThemeId)>,
    downloads: tokio::sync::Semaphore,
}

impl std::fmt::Debug for ThemePreviews {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThemePreviews").finish_non_exhaustive()
    }
}

impl ThemePreviews {
    #[must_use]
    pub fn new(
        catalog: Arc<CatalogService>,
        fetcher: Arc<dyn CatalogFetcher>,
        cache: Arc<dyn PreviewCache>,
    ) -> Self {
        Self {
            catalog,
            fetcher,
            cache,
            flights: Flights::default(),
            downloads: tokio::sync::Semaphore::new(MAX_PARALLEL_PREVIEWS),
        }
    }

    /// The image the source names for the theme, from the cache or the address of the index.
    pub async fn get(&self, source: &str, theme: &str) -> Result<Preview, AppError> {
        let target = self.catalog.preview_of(source, theme).await?;
        let key = cache_key(&target.theme, target.preview.fingerprint());
        let version = target.preview.version().to_owned();
        if let Some(found) = self.cached(&target.source, &key, &version).await {
            return Ok(found);
        }
        let _flight = self
            .flights
            .enter((target.source.clone(), target.theme.clone()))
            .await;
        // whoever was ahead may have loaded it meanwhile
        if let Some(found) = self.cached(&target.source, &key, &version).await {
            return Ok(found);
        }
        let bytes = {
            let _permit = self.downloads.acquire().await.map_err(AppError::internal)?;
            self.fetcher
                .fetch(target.preview.url(), MAX_PREVIEW_BYTES)
                .await
                .map_err(|error| match error {
                    FetchError::TooLarge => too_large(),
                    other => fetch_failed(other, "the preview"),
                })?
        };
        if bytes.len() as u64 > MAX_PREVIEW_BYTES {
            return Err(too_large());
        }
        let image = PreviewImage::sniff(&bytes).ok_or_else(not_an_image)?;
        // the person may have removed the source meanwhile
        self.catalog
            .preview_of(target.source.as_str(), target.theme.as_str())
            .await?;
        let stored = self.cache.put(&target.source, &key, bytes).await?;
        if let Some(error) = stored.error {
            tracing::warn!(%error, source = %target.source, "cannot cache a theme preview");
        }
        Ok(Preview {
            data: stored.bytes,
            content_type: image.content_type(),
            version,
        })
    }

    /// Drops what was kept for a source that is not one any more.
    pub async fn forget(&self, source: &str) {
        let Some(id) = SourceId::parse(source) else {
            return;
        };
        if let Err(error) = self.cache.clear(&id).await {
            tracing::warn!(%error, source = %id, "cannot empty the previews of a source");
        }
    }

    /// A kept image counts only if it still is one; anything else is fetched anew.
    async fn cached(&self, source: &SourceId, key: &str, version: &str) -> Option<Preview> {
        let data = self.cache.get(source, key).await?;
        if data.len() as u64 > MAX_PREVIEW_BYTES {
            return None;
        }
        let image = PreviewImage::sniff(&data)?;
        Some(Preview {
            data,
            content_type: image.content_type(),
            version: version.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests;
