//! The asset proxy of themes: which addresses a theme may load and what a response is served as.

use url::Url;

use crate::{
    CatalogError, CatalogUrl, EndpointHost, PackageError, Reach, asset_segments, content_type,
    host_allowed,
};

/// Largest resource, and largest file taken out of an archive.
pub const MAX_ASSET_BYTES: u64 = 10 * 1024 * 1024;
/// Largest archive a file may be taken from.
pub const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_REDIRECTS: usize = 5;
/// Cached bytes kept per theme.
pub const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;
/// Cached files kept per theme.
pub const MAX_CACHE_FILES: usize = 4096;

const OPAQUE_TYPE: &str = "application/octet-stream";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssetUrlError {
    #[error(transparent)]
    Invalid(#[from] CatalogError),
    #[error("the theme does not declare the domain {host}")]
    NotDeclared { host: String },
}

/// An https address on a public host whose domain the theme declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetUrl(CatalogUrl);

impl AssetUrl {
    pub fn parse(raw: &str, network: &[String]) -> Result<Self, AssetUrlError> {
        let url = CatalogUrl::parse(raw, Reach::Public)?;
        let host = match url.endpoint().ok_or(CatalogError::NoHost)?.host {
            EndpointHost::Name(name) => name,
            EndpointHost::Address(address) => address.to_string(),
        };
        if !host_allowed(network, &host) {
            return Err(AssetUrlError::NotDeclared { host });
        }
        Ok(Self(url))
    }

    /// Where a redirect from this address leads, checked like a first address.
    pub fn redirect(&self, location: &str, network: &[String]) -> Result<Self, AssetUrlError> {
        let base = Url::parse(self.0.as_str()).map_err(|_| CatalogError::Malformed)?;
        let next = base
            .join(location.trim())
            .map_err(|_| CatalogError::Malformed)?;
        Self::parse(next.as_str(), network)
    }

    #[must_use]
    pub const fn catalog_url(&self) -> &CatalogUrl {
        &self.0
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// The path of the address, which names the type of the resource.
    #[must_use]
    pub fn path(&self) -> String {
        Url::parse(self.0.as_str()).map_or_else(|_| String::new(), |url| url.path().to_owned())
    }
}

/// The path of a file inside an archive, plain names only.
pub fn check_asset_entry(entry: &str) -> Result<(), PackageError> {
    asset_segments(entry).map(|_| ())
}

/// A sha256 checksum as 64 lowercase hex digits.
#[must_use]
pub fn parse_sha256(text: &str) -> Option<String> {
    (text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| text.to_ascii_lowercase())
}

/// Type of a foreign resource by its extension; text that a page could run or apply is opaque.
#[must_use]
pub fn foreign_content_type(path: &str) -> &'static str {
    let kind = content_type(path);
    // a script or style from outside never passes the checks of a package, so it must not run
    if kind.starts_with("text/") {
        OPAQUE_TYPE
    } else {
        kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network() -> Vec<String> {
        vec![
            String::from("cdn.example.com"),
            String::from("*.assets.test"),
        ]
    }

    fn refused(raw: &str) -> AssetUrlError {
        AssetUrl::parse(raw, &network()).unwrap_err()
    }

    #[test]
    fn a_declared_https_host_is_accepted() {
        let url = AssetUrl::parse("https://CDN.example.com/a/b.png?x=1#top", &network()).unwrap();
        assert_eq!(url.as_str(), "https://cdn.example.com/a/b.png?x=1");
        assert_eq!(url.path(), "/a/b.png");
        assert!(AssetUrl::parse("https://x.assets.test/f.woff2", &network()).is_ok());
        assert!(AssetUrl::parse("https://cdn.example.com./f.png", &network()).is_ok());
    }

    #[test]
    fn a_host_that_is_not_declared_is_refused_by_name() {
        for raw in [
            "https://example.com/a.png",
            "https://evilcdn.example.com/a.png",
            "https://cdn.example.com.evil.test/a.png",
            "https://assets.test/a.png",
            "https://evil-assets.test/a.png",
            // a Cyrillic a or c in place of the Latin letter is another host
            "https://cdn.ex\u{430}mple.com/a.png",
            "https://\u{441}dn.example.com/a.png",
        ] {
            assert!(
                matches!(refused(raw), AssetUrlError::NotDeclared { .. }),
                "{raw}"
            );
        }
        assert_eq!(
            refused("https://other.test/a.png").to_string(),
            "the theme does not declare the domain other.test"
        );
    }

    #[test]
    fn only_public_https_without_credentials_is_taken() {
        for raw in [
            "http://cdn.example.com/a.png",
            "ftp://cdn.example.com/a.png",
            "file:///etc/passwd",
            "https://user:pass@cdn.example.com/a.png",
            "https://cdn.example.com:8443/a.png",
            "https://cdn.example.com/a b.png",
            "",
            "not a url",
        ] {
            assert!(matches!(refused(raw), AssetUrlError::Invalid(_)), "{raw}");
        }
    }

    #[test]
    fn a_declared_address_is_still_refused_when_it_is_not_public() {
        let network: Vec<String> = ["127.0.0.1", "169.254.169.254", "10.0.0.1", "app.internal"]
            .map(String::from)
            .into();
        for raw in [
            "https://127.0.0.1/a.png",
            "https://169.254.169.254/latest/meta-data",
            "https://10.0.0.1/a.png",
            "https://[::1]/a.png",
            "https://2130706433/a.png",
            "https://0x7f.1/a.png",
            "https://app.internal/a.png",
        ] {
            assert!(
                matches!(
                    AssetUrl::parse(raw, &network),
                    Err(AssetUrlError::Invalid(CatalogError::Private))
                ),
                "{raw}"
            );
        }
    }

    #[test]
    fn a_redirect_is_checked_like_a_first_address() {
        let from = AssetUrl::parse("https://cdn.example.com/a/b.png", &network()).unwrap();
        let next = from.redirect("../c.png", &network()).unwrap();
        assert_eq!(next.as_str(), "https://cdn.example.com/c.png");
        let other = from
            .redirect("https://x.assets.test/d.png", &network())
            .unwrap();
        assert_eq!(other.as_str(), "https://x.assets.test/d.png");
        assert!(matches!(
            from.redirect("https://evil.test/d.png", &network()),
            Err(AssetUrlError::NotDeclared { .. })
        ));
        assert!(matches!(
            from.redirect("http://cdn.example.com/d.png", &network()),
            Err(AssetUrlError::Invalid(CatalogError::Scheme))
        ));
        assert!(matches!(
            from.redirect("//127.0.0.1/d.png", &network()),
            Err(AssetUrlError::Invalid(_) | AssetUrlError::NotDeclared { .. })
        ));
        assert!(from.redirect("https://[", &network()).is_err());
    }

    #[test]
    fn entry_paths_are_plain_names() {
        assert!(check_asset_entry("assets/minecraft/textures/block/stone.png").is_ok());
        for entry in [
            "",
            "/a.png",
            "../a.png",
            "a/../b.png",
            "a//b.png",
            "./a.png",
            "a\\b.png",
            "C:/a.png",
            "a/b/",
            "a\0b.png",
            "a/b.",
            "a/b ",
        ] {
            assert!(check_asset_entry(entry).is_err(), "{entry:?}");
        }
        assert!(check_asset_entry(&"a/".repeat(257)).is_err());
        let longest = format!("{}.png", "a".repeat(512 - 4));
        assert!(check_asset_entry(&longest).is_ok());
        assert!(check_asset_entry(&format!("a{longest}")).is_err());
    }

    #[test]
    fn a_checksum_is_64_hex_digits() {
        let sum = "AB".repeat(32);
        assert_eq!(parse_sha256(&sum), Some("ab".repeat(32)));
        for text in [&sum[..63], &format!("{sum}0"), &"g".repeat(64), ""] {
            assert_eq!(parse_sha256(text), None, "{text}");
        }
    }

    #[test]
    fn foreign_scripts_styles_and_markup_are_opaque() {
        assert_eq!(foreign_content_type("/a/b.PNG"), "image/png");
        assert_eq!(foreign_content_type("/f.woff2"), "font/woff2");
        assert_eq!(
            foreign_content_type("pack.json"),
            "application/json; charset=utf-8"
        );
        for path in [
            "/x.js", "/x.mjs", "/x.css", "/x.html", "/x.svgz", "/x", "/dir/",
        ] {
            assert_eq!(foreign_content_type(path), OPAQUE_TYPE, "{path}");
        }
    }
}
