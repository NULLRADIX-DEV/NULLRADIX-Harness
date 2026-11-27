//! Preview images of catalog entries: where they come from and what may be served.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

use crate::CatalogUrl;

/// Largest preview image a source may serve.
pub const MAX_PREVIEW_BYTES: u64 = 2 * 1024 * 1024;
/// What the cache keeps of one source's previews.
pub const MAX_PREVIEW_CACHE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_PREVIEW_CACHE_FILES: usize = 1024;
/// Characters of the fingerprint a page adds to the address, so a new image is fetched anew.
pub const PREVIEW_VERSION_CHARS: usize = 16;

/// The preview an index entry names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPreview {
    url: CatalogUrl,
    fingerprint: String,
}

impl CatalogPreview {
    /// The image changes when its address or the package checksum of the entry changes.
    #[must_use]
    pub fn new(url: CatalogUrl, sha256: &str) -> Self {
        let digest = Sha256::digest(format!("{}\n{sha256}", url.as_str()).as_bytes());
        let fingerprint = digest.iter().fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        });
        Self { url, fingerprint }
    }

    #[must_use]
    pub const fn url(&self) -> &CatalogUrl {
        &self.url
    }

    /// Lower-case hex of the address and the checksum.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The start of the fingerprint, short enough for an address.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.fingerprint[..PREVIEW_VERSION_CHARS]
    }
}

/// The image kinds that are served; SVG and animated or exotic formats are not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewImage {
    Png,
    Jpeg,
    Webp,
}

impl PreviewImage {
    /// The kind by the first bytes, whatever the source called the file.
    #[must_use]
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
            Some(Self::Webp)
        } else {
            None
        }
    }

    #[must_use]
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Reach;

    fn url(raw: &str) -> CatalogUrl {
        CatalogUrl::parse(raw, Reach::Public).unwrap()
    }

    #[test]
    fn the_fingerprint_follows_the_address_and_the_checksum() {
        let sum = "ab".repeat(32);
        let one = CatalogPreview::new(url("https://example.com/a.png"), &sum);
        let same = CatalogPreview::new(url("HTTPS://Example.com/a.png"), &sum);
        let moved = CatalogPreview::new(url("https://example.com/b.png"), &sum);
        let repacked = CatalogPreview::new(url("https://example.com/a.png"), &"cd".repeat(32));
        assert_eq!(one.fingerprint(), same.fingerprint());
        assert_ne!(one.fingerprint(), moved.fingerprint());
        assert_ne!(one.fingerprint(), repacked.fingerprint());
        assert_eq!(one.fingerprint().len(), 64);
        assert_eq!(one.version(), &one.fingerprint()[..PREVIEW_VERSION_CHARS]);
    }

    #[test]
    fn png_jpeg_and_webp_are_known_by_their_first_bytes() {
        assert_eq!(
            PreviewImage::sniff(b"\x89PNG\r\n\x1a\n\0\0"),
            Some(PreviewImage::Png)
        );
        assert_eq!(
            PreviewImage::sniff(b"\xff\xd8\xff\xe0\0\x10JFIF"),
            Some(PreviewImage::Jpeg)
        );
        assert_eq!(
            PreviewImage::sniff(b"RIFF\x1a\0\0\0WEBPVP8 "),
            Some(PreviewImage::Webp)
        );
        assert_eq!(PreviewImage::Png.content_type(), "image/png");
        assert_eq!(PreviewImage::Jpeg.content_type(), "image/jpeg");
        assert_eq!(PreviewImage::Webp.content_type(), "image/webp");
    }

    #[test]
    fn everything_else_is_no_preview() {
        for bytes in [
            &b""[..],
            b"\x89PNG",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>",
            b"<?xml version=\"1.0\"?><svg/>",
            b"<!doctype html><script>alert(1)</script>",
            b"GIF89a-animated",
            b"RIFF\0\0\0\0WAVEfmt ",
            b"RIFF\0\0\0\0AVI LIST",
            b"\0\0\0\x1cftypavif",
            b"PK\x03\x04",
        ] {
            assert_eq!(PreviewImage::sniff(bytes), None, "{bytes:?}");
        }
    }
}
