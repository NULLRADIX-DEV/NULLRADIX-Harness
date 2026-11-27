//! Theme catalogs: the addresses they are fetched from and the entries they offer.

use std::fmt::{self, Write as _};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use sha2::{Digest, Sha256};
use url::{Host, Url};

use crate::theme::SDK_VERSION;
use crate::theme_preview::CatalogPreview;
use crate::{ThemeId, Timestamp};

pub const MAX_SOURCES: usize = 50;
pub const MAX_URL_CHARS: usize = 2048;
/// Largest index a source may serve.
pub const MAX_INDEX_BYTES: u64 = 1024 * 1024;
pub const MAX_INDEX_ENTRIES: usize = 500;
pub const MAX_DESCRIPTION_CHARS: usize = 2000;
/// Longest version or SDK range an entry may state.
pub const MAX_VERSION_CHARS: usize = 128;
const SOURCE_ID_CHARS: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    #[error("the address is empty")]
    Empty,
    #[error("the address is longer than {MAX_URL_CHARS} characters")]
    TooLong,
    #[error("the address holds spaces or control characters")]
    Characters,
    #[error("the address is not a valid URL")]
    Malformed,
    #[error("the address must start with https://")]
    Scheme,
    #[error("the address has no host")]
    NoHost,
    #[error("the address holds credentials")]
    Credentials,
    #[error("the address points to a local or private host")]
    Private,
    #[error("only the default https port is allowed")]
    Port,
}

/// Where addresses may point to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// https to public hosts on the default port only.
    Public,
    /// Also http, local and private hosts and any port; for self-hosted catalogs.
    Anywhere,
}

/// An address a catalog or a package is fetched from, checked for its reach.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CatalogUrl(String);

impl CatalogUrl {
    /// Normalizes the address and refuses everything outside `reach`.
    pub fn parse(raw: &str, reach: Reach) -> Result<Self, CatalogError> {
        let text = raw.trim();
        if text.is_empty() {
            return Err(CatalogError::Empty);
        }
        if text.chars().count() > MAX_URL_CHARS {
            return Err(CatalogError::TooLong);
        }
        if text.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(CatalogError::Characters);
        }
        let mut url = Url::parse(text).map_err(|_| CatalogError::Malformed)?;
        match (url.scheme(), reach) {
            ("https", _) | ("http", Reach::Anywhere) => {}
            _ => return Err(CatalogError::Scheme),
        }
        let Some(host) = url.host() else {
            return Err(CatalogError::NoHost);
        };
        if !url.username().is_empty() || url.password().is_some() {
            return Err(CatalogError::Credentials);
        }
        if reach == Reach::Public {
            if !host_is_public(&host) {
                return Err(CatalogError::Private);
            }
            if url.port().is_some() {
                return Err(CatalogError::Port);
            }
        }
        url.set_fragment(None);
        Ok(Self(url.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The host and port a request goes to.
    #[must_use]
    pub fn endpoint(&self) -> Option<Endpoint> {
        let url = Url::parse(&self.0).ok()?;
        let port = url.port_or_known_default()?;
        let host = match url.host()? {
            Host::Domain(name) => EndpointHost::Name(name.to_ascii_lowercase()),
            Host::Ipv4(address) => EndpointHost::Address(IpAddr::V4(address)),
            Host::Ipv6(address) => EndpointHost::Address(IpAddr::V6(address)),
        };
        Some(Endpoint { host, port })
    }
}

/// The host of a request: a name that still needs resolving or an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointHost {
    Name(String),
    Address(IpAddr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: EndpointHost,
    pub port: u16,
}

impl fmt::Display for CatalogUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn host_is_public(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            let local_name = name == "localhost"
                || [".localhost", ".local", ".internal"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
                || !name.contains('.');
            !local_name
        }
        Host::Ipv4(address) => is_public_address(IpAddr::V4(*address)),
        Host::Ipv6(address) => is_public_address(IpAddr::V6(*address)),
    }
}

/// Whether the address belongs to the public internet; DNS answers are checked with it too.
#[must_use]
pub fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => public_v4(address),
        IpAddr::V6(address) => public_v6(address),
    }
}

fn public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    let shared = a == 100 && (64..128).contains(&b);
    let protocol = a == 192 && b == 0 && c == 0;
    let benchmark = a == 198 && (b == 18 || b == 19);
    let reserved = a >= 240;
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_documentation()
        || address.is_multicast()
        || a == 0
        || shared
        || protocol
        || benchmark
        || reserved)
}

fn public_v6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return public_v4(mapped);
    }
    let segments = address.segments();
    // NAT64 and 6to4 carry an IPv4 address that must be public itself
    let embedded = match segments {
        [0x64, 0xff9b, 0, 0, 0, 0, high, low] | [0x2002, high, low, ..] => Some((high, low)),
        _ => None,
    };
    if let Some((high, low)) = embedded {
        let [a, b] = high.to_be_bytes();
        let [c, d] = low.to_be_bytes();
        return public_v4(Ipv4Addr::new(a, b, c, d));
    }
    // only global unicast 2000::/3 counts; everything else is special use
    let global_unicast = segments[0] & 0xe000 == 0x2000;
    let teredo = segments[0] == 0x2001 && segments[1] == 0;
    let documentation = (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || (segments[0] == 0x3fff && segments[1] & 0xf000 == 0);
    global_unicast && !(teredo || documentation)
}

/// Stable id of a source: the first characters of the hash of its address.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(String);

impl SourceId {
    #[must_use]
    pub fn of(url: &CatalogUrl) -> Self {
        let digest = Sha256::digest(url.as_str().as_bytes());
        let mut text = String::new();
        for byte in digest.iter().take(SOURCE_ID_CHARS / 2) {
            let _ = write!(text, "{byte:02x}");
        }
        Self(text)
    }

    /// Reads an id a client sent; anything else than hex of the right length is refused.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        (text.len() == SOURCE_ID_CHARS
            && text
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
        .then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An address the person added to look for themes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSource {
    id: SourceId,
    url: CatalogUrl,
    added_at: Timestamp,
}

impl CatalogSource {
    #[must_use]
    pub fn new(url: CatalogUrl, added_at: Timestamp) -> Self {
        Self {
            id: SourceId::of(&url),
            url,
            added_at,
        }
    }

    #[must_use]
    pub const fn id(&self) -> &SourceId {
        &self.id
    }

    #[must_use]
    pub const fn url(&self) -> &CatalogUrl {
        &self.url
    }

    #[must_use]
    pub const fn added_at(&self) -> Timestamp {
        self.added_at
    }
}

/// The fields of one index entry as the index states them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryDraft {
    pub id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub sdk_version: Option<String>,
    pub download_url: Option<String>,
    pub sha256: Option<String>,
    pub preview: Option<String>,
    pub description: Option<String>,
}

/// A theme a source offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: ThemeId,
    pub name: String,
    pub version: String,
    pub sdk_version: String,
    /// Whether the SDK range accepts this build.
    pub compatible: bool,
    pub download_url: CatalogUrl,
    /// Lower-case hex of the package.
    pub sha256: String,
    /// The image the source offers for the entry.
    pub preview: Option<CatalogPreview>,
    pub description: Option<String>,
}

fn text<'a>(value: Option<&'a String>, name: &str) -> Result<&'a str, String> {
    value
        .map(String::as_str)
        .ok_or_else(|| format!("{name} is missing"))
}

/// Whether the text has no control characters or bidi overrides; `newline` allows line breaks.
fn printable(text: &str, newline: bool) -> bool {
    text.chars().all(|c| {
        (!c.is_control() || (newline && c == '\n'))
            && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    })
}

impl CatalogEntry {
    /// Checks the fields; the first problem is the reason the entry is skipped.
    pub fn build(draft: &EntryDraft, reach: Reach) -> Result<Self, String> {
        let id = ThemeId::parse(text(draft.id.as_ref(), "id")?)
            .map_err(|error| format!("id: {error}"))?;
        let name = text(draft.name.as_ref(), "name")?;
        if name.trim().is_empty() {
            return Err(String::from("name is empty"));
        }
        if name.chars().count() > 1000 {
            return Err(String::from("name is longer than 1000 characters"));
        }
        if !printable(name, false) {
            return Err(String::from("name holds control or direction characters"));
        }
        let version = text(draft.version.as_ref(), "version")?;
        if version.chars().count() > MAX_VERSION_CHARS {
            return Err(format!(
                "version is longer than {MAX_VERSION_CHARS} characters"
            ));
        }
        semver::Version::parse(version)
            .map_err(|_| String::from("version is not a semver version such as 1.2.3"))?;
        let sdk_version = text(draft.sdk_version.as_ref(), "sdkVersion")?;
        if sdk_version.chars().count() > MAX_VERSION_CHARS {
            return Err(format!(
                "sdkVersion is longer than {MAX_VERSION_CHARS} characters"
            ));
        }
        let range = semver::VersionReq::parse(sdk_version)
            .map_err(|_| String::from("sdkVersion is not a semver range such as ^1.0"))?;
        let download_url =
            CatalogUrl::parse(text(draft.download_url.as_ref(), "downloadUrl")?, reach)
                .map_err(|error| format!("downloadUrl: {error}"))?;
        let sha256 = text(draft.sha256.as_ref(), "sha256")?;
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(String::from("sha256 must be 64 hex characters"));
        }
        let sha256 = sha256.to_ascii_lowercase();
        let preview = draft
            .preview
            .as_deref()
            .map(|raw| {
                CatalogUrl::parse(raw, reach)
                    .map(|url| CatalogPreview::new(url, &sha256))
                    .map_err(|error| format!("preview: {error}"))
            })
            .transpose()?;
        if draft
            .description
            .as_ref()
            .is_some_and(|text| text.chars().count() > MAX_DESCRIPTION_CHARS)
        {
            return Err(format!(
                "description is longer than {MAX_DESCRIPTION_CHARS} characters"
            ));
        }
        if draft
            .description
            .as_ref()
            .is_some_and(|text| !printable(text, true))
        {
            return Err(String::from(
                "description holds control or direction characters",
            ));
        }
        let sdk = semver::Version::parse(SDK_VERSION)
            .map_err(|_| String::from("the SDK version of this build is invalid"))?;
        Ok(Self {
            id,
            name: name.to_owned(),
            version: version.to_owned(),
            sdk_version: sdk_version.to_owned(),
            compatible: range.matches(&sdk),
            download_url,
            sha256,
            preview,
            description: draft.description.clone(),
        })
    }
}

/// What a catalog entry means for this installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    NotInstalled,
    /// Installed at this version, which is not older than the entry.
    Installed(String),
    /// Installed at this older version.
    Update(String),
    /// Installed at this version from another source or by upload.
    OtherSource(String),
}

impl Availability {
    /// Compares by semver; a theme from another source is never an update.
    #[must_use]
    pub fn of(installed: Option<&str>, from_this_source: bool, offered: &str) -> Self {
        let Some(installed) = installed else {
            return Self::NotInstalled;
        };
        if !from_this_source {
            return Self::OtherSource(installed.to_owned());
        }
        match (
            semver::Version::parse(installed),
            semver::Version::parse(offered),
        ) {
            (Ok(have), Ok(offer)) if offer <= have => Self::Installed(installed.to_owned()),
            _ => Self::Update(installed.to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(raw: &str) -> Result<CatalogUrl, CatalogError> {
        CatalogUrl::parse(raw, Reach::Public)
    }

    #[test]
    fn a_public_https_address_is_normalized() {
        let url = public("  HTTPS://Example.COM/themes/index.json#top ").unwrap();
        assert_eq!(url.as_str(), "https://example.com/themes/index.json");
    }

    #[test]
    fn everything_but_public_https_is_refused() {
        let cases = [
            ("", CatalogError::Empty),
            ("http://example.com/i.json", CatalogError::Scheme),
            ("ftp://example.com/i.json", CatalogError::Scheme),
            ("file:///etc/passwd", CatalogError::Scheme),
            ("example.com/i.json", CatalogError::Malformed),
            ("https://", CatalogError::Malformed),
            ("https://u:p@example.com/", CatalogError::Credentials),
            ("https://u@example.com/", CatalogError::Credentials),
            ("https://example.com/a b", CatalogError::Characters),
            ("https://example.com:8443/", CatalogError::Port),
            ("https://localhost/i.json", CatalogError::Private),
            ("https://app.localhost/i.json", CatalogError::Private),
            ("https://printer.local/i.json", CatalogError::Private),
            ("https://intranet/i.json", CatalogError::Private),
            ("https://127.0.0.1/i.json", CatalogError::Private),
            ("https://2130706433/i.json", CatalogError::Private),
            ("https://0x7f.1/i.json", CatalogError::Private),
            ("https://10.0.0.5/i.json", CatalogError::Private),
            ("https://192.168.1.1/i.json", CatalogError::Private),
            ("https://169.254.169.254/latest", CatalogError::Private),
            ("https://100.64.0.1/i.json", CatalogError::Private),
            ("https://[::1]/i.json", CatalogError::Private),
            ("https://[fd00::1]/i.json", CatalogError::Private),
            ("https://[fe80::1]/i.json", CatalogError::Private),
            ("https://[::ffff:127.0.0.1]/i.json", CatalogError::Private),
            ("https://[64:ff9b::7f00:1]/i.json", CatalogError::Private),
            ("https://[::7f00:1]/i.json", CatalogError::Private),
            ("https://[fec0::1]/i.json", CatalogError::Private),
            ("https://[64:ff9b:1::1]/i.json", CatalogError::Private),
            ("https://[2001::1]/i.json", CatalogError::Private),
            ("https://[100::1]/i.json", CatalogError::Private),
        ];
        for (raw, expected) in cases {
            assert_eq!(public(raw).unwrap_err(), expected, "{raw}");
        }
    }

    #[test]
    fn the_endpoint_names_host_and_port() {
        let endpoint = |raw: &str| {
            CatalogUrl::parse(raw, Reach::Anywhere)
                .unwrap()
                .endpoint()
                .unwrap()
        };
        assert_eq!(
            endpoint("https://Example.com/a"),
            Endpoint {
                host: EndpointHost::Name(String::from("example.com")),
                port: 443
            }
        );
        assert_eq!(
            endpoint("http://127.0.0.1:8080/a"),
            Endpoint {
                host: EndpointHost::Address("127.0.0.1".parse().unwrap()),
                port: 8080
            }
        );
        assert_eq!(
            endpoint("https://[::1]/a").host,
            EndpointHost::Address("::1".parse().unwrap())
        );
    }

    #[test]
    fn an_address_is_limited_in_length() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_CHARS));
        assert_eq!(public(&long).unwrap_err(), CatalogError::TooLong);
    }

    #[test]
    fn public_addresses_pass_and_private_ones_do_not() {
        assert!(is_public_address("8.8.8.8".parse().unwrap()));
        assert!(is_public_address("2606:4700::1111".parse().unwrap()));
        for text in [
            "0.0.0.0",
            "127.0.0.1",
            "172.16.0.1",
            "198.18.0.1",
            "240.0.0.1",
            "::",
            "::1",
            "::7f00:1",
            "fec0::1",
            "64:ff9b:1::1",
            "2001::1",
            "100::1",
            "3fff::1",
        ] {
            assert!(!is_public_address(text.parse().unwrap()), "{text}");
        }
    }

    #[test]
    fn anywhere_allows_http_local_hosts_and_ports_but_no_credentials() {
        let url = CatalogUrl::parse("http://127.0.0.1:8080/i.json", Reach::Anywhere).unwrap();
        assert_eq!(url.as_str(), "http://127.0.0.1:8080/i.json");
        assert_eq!(
            CatalogUrl::parse("http://u:p@127.0.0.1/", Reach::Anywhere).unwrap_err(),
            CatalogError::Credentials
        );
        assert_eq!(
            CatalogUrl::parse("ftp://127.0.0.1/", Reach::Anywhere).unwrap_err(),
            CatalogError::Scheme
        );
    }

    #[test]
    fn a_source_id_follows_the_address() {
        let one = CatalogSource::new(
            public("https://a.example.com/i.json").unwrap(),
            Timestamp::from_millis(1),
        );
        let same = CatalogSource::new(
            public("HTTPS://A.example.com/i.json").unwrap(),
            Timestamp::from_millis(2),
        );
        let other = CatalogSource::new(
            public("https://b.example.com/i.json").unwrap(),
            Timestamp::from_millis(1),
        );
        assert_eq!(one.id(), same.id());
        assert_ne!(one.id(), other.id());
        assert_eq!(one.id().as_str().len(), 12);
        assert_eq!(SourceId::parse(one.id().as_str()).as_ref(), Some(one.id()));
        assert_eq!(SourceId::parse("ABCDEF123456"), None);
        assert_eq!(SourceId::parse("abc"), None);
    }

    type Change = Box<dyn Fn(&mut EntryDraft)>;

    fn draft() -> EntryDraft {
        EntryDraft {
            id: Some(String::from("ocean")),
            name: Some(String::from("Ocean")),
            version: Some(String::from("1.2.0")),
            sdk_version: Some(String::from("^2.0")),
            download_url: Some(String::from("https://example.com/ocean.nullradix")),
            sha256: Some("AB".repeat(32)),
            preview: None,
            description: Some(String::from("Blue")),
        }
    }

    fn build(draft: &EntryDraft) -> Result<CatalogEntry, String> {
        CatalogEntry::build(draft, Reach::Public)
    }

    #[test]
    fn a_valid_entry_is_compatible_and_lower_cases_the_checksum() {
        let entry = build(&draft()).unwrap();
        assert!(entry.compatible);
        assert_eq!(entry.sha256, "ab".repeat(32));
        assert_eq!(entry.id.as_str(), "ocean");
    }

    #[test]
    fn an_entry_without_a_preview_has_none() {
        assert_eq!(build(&draft()).unwrap().preview, None);
    }

    #[test]
    fn a_preview_is_a_public_https_address_tied_to_the_checksum() {
        let mut named = draft();
        named.preview = Some(String::from("HTTPS://Example.com/ocean.png#x"));
        let entry = build(&named).unwrap();
        let preview = entry.preview.unwrap();
        assert_eq!(preview.url().as_str(), "https://example.com/ocean.png");
        let mut repacked = named.clone();
        repacked.sha256 = Some("cd".repeat(32));
        let again = build(&repacked).unwrap().preview.unwrap();
        assert_ne!(preview.fingerprint(), again.fingerprint());
        // the checksum counts in lower case
        let mut upper = named;
        upper.sha256 = Some("ab".repeat(32));
        assert_eq!(build(&upper).unwrap().preview.unwrap(), preview);
    }

    #[test]
    fn a_preview_must_pass_the_rules_of_the_reach() {
        for bad in [
            "http://example.com/a.png",
            "https://10.0.0.1/a.png",
            "https://localhost/a.png",
            "https://u:p@example.com/a.png",
            "https://example.com:8443/a.png",
            "file:///etc/passwd",
            "",
        ] {
            let mut entry = draft();
            entry.preview = Some(bad.to_owned());
            let reason = build(&entry).unwrap_err();
            assert!(reason.starts_with("preview: "), "{bad}: {reason}");
        }
        let mut local = draft();
        local.preview = Some(String::from("http://127.0.0.1:8080/a.png"));
        assert!(CatalogEntry::build(&local, Reach::Anywhere).is_ok());
    }

    #[test]
    fn a_description_may_have_line_breaks() {
        let mut entry = draft();
        entry.description = Some(String::from("Blue\nand calm"));
        assert!(build(&entry).is_ok());
    }

    #[test]
    fn compatibility_is_decided_by_the_sdk_version_of_the_theme_module() {
        let version = semver::Version::parse(SDK_VERSION).unwrap();
        let mut exact = draft();
        exact.sdk_version = Some(format!("={version}"));
        assert!(build(&exact).unwrap().compatible);
        let mut next = draft();
        next.sdk_version = Some(format!(">{version}"));
        assert!(!build(&next).unwrap().compatible);
    }

    #[test]
    fn an_entry_for_another_sdk_stays_but_is_not_compatible() {
        let mut other = draft();
        other.sdk_version = Some(String::from("^3.0"));
        assert!(!build(&other).unwrap().compatible);
    }

    #[test]
    fn a_bad_entry_says_why() {
        let broken: Vec<(Change, &str)> = vec![
            (Box::new(|d| d.id = None), "id is missing"),
            (Box::new(|d| d.id = Some(String::from("Bad Id"))), "id: "),
            (
                Box::new(|d| d.name = Some(String::from("  "))),
                "name is empty",
            ),
            (
                Box::new(|d| d.version = Some(String::from("one"))),
                "version is not",
            ),
            (
                Box::new(|d| d.sdk_version = Some(String::from("nope"))),
                "sdkVersion is not",
            ),
            (
                Box::new(|d| d.download_url = Some(String::from("http://example.com/a"))),
                "downloadUrl: ",
            ),
            (
                Box::new(|d| d.download_url = Some(String::from("https://10.0.0.1/a"))),
                "downloadUrl: ",
            ),
            (
                Box::new(|d| d.sha256 = Some(String::from("abc"))),
                "sha256 must",
            ),
            (
                Box::new(|d| d.sha256 = Some("zz".repeat(32))),
                "sha256 must",
            ),
            (
                Box::new(|d| d.description = Some("x".repeat(MAX_DESCRIPTION_CHARS + 1))),
                "description is longer",
            ),
            (
                Box::new(|d| d.name = Some(String::from("Oc\u{1b}ean"))),
                "name holds",
            ),
            (
                Box::new(|d| d.name = Some(String::from("Oc\u{202e}ean"))),
                "name holds",
            ),
            (
                Box::new(|d| d.name = Some(String::from("a\nb"))),
                "name holds",
            ),
            (
                Box::new(|d| d.description = Some(String::from("a\r\nb"))),
                "description holds",
            ),
            (
                Box::new(|d| d.description = Some(String::from("a\u{2066}b"))),
                "description holds",
            ),
            (
                Box::new(|d| d.version = Some(format!("1.0.0-{}", "a".repeat(MAX_VERSION_CHARS)))),
                "version is longer",
            ),
            (
                Box::new(|d| {
                    d.sdk_version = Some(format!("^1.0.0-{}", "a".repeat(MAX_VERSION_CHARS)));
                }),
                "sdkVersion is longer",
            ),
        ];
        for (change, expected) in broken {
            let mut entry = draft();
            change(&mut entry);
            let reason = build(&entry).unwrap_err();
            assert!(reason.starts_with(expected), "{reason} vs {expected}");
        }
    }

    #[test]
    fn availability_compares_versions() {
        assert_eq!(
            Availability::of(None, false, "1.0.0"),
            Availability::NotInstalled
        );
        assert_eq!(
            Availability::of(Some("1.0.0"), true, "1.0.0"),
            Availability::Installed(String::from("1.0.0"))
        );
        assert_eq!(
            Availability::of(Some("1.1.0"), true, "1.0.0"),
            Availability::Installed(String::from("1.1.0"))
        );
        assert_eq!(
            Availability::of(Some("1.0.0"), true, "1.0.1"),
            Availability::Update(String::from("1.0.0"))
        );
        assert_eq!(
            Availability::of(Some("1.0.0"), true, "2.0.0-beta.1"),
            Availability::Update(String::from("1.0.0"))
        );
        assert_eq!(
            Availability::of(Some("1.0.0"), false, "1.0.1"),
            Availability::OtherSource(String::from("1.0.0"))
        );
    }
}
