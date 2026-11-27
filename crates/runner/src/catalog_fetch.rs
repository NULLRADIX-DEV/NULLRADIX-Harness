//! Downloads catalog indexes and theme packages through `curl`, which every supported system ships.

use std::fmt::Write as _;
use std::net::IpAddr;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_app::theme_catalog::{CatalogFetcher, FetchError};
use nullradix_app::theme_fetch::{AssetFetcher, Hop};
use nullradix_domain::{
    CatalogUrl, EndpointHost, MAX_ASSET_BYTES, MAX_INDEX_BYTES, Reach, is_public_address,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Whole download of a package, across redirects and the lookup of host names.
const PACKAGE_LIMIT: Duration = Duration::from_secs(60);
/// Indexes are small; a slow source must not hold up the others for long.
const INDEX_LIMIT: Duration = Duration::from_secs(20);
const CONNECT_LIMIT_SECONDS: u64 = 10;
const MAX_REDIRECTS: usize = 3;
/// One request for a resource of a theme; the redirects of the proxy have a budget of their own.
const ASSET_LIMIT: Duration = Duration::from_secs(30);
/// One request for an archive a file is taken from.
const ARCHIVE_LIMIT: Duration = Duration::from_secs(120);
/// Room after the body for the status line the request writes behind it.
const TRAILER_ROOM: u64 = 16 * 1024;
const USER_AGENT: &str = "nullradix-catalog";

/// Looks up the addresses of a host name.
#[async_trait]
trait Resolve: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>>;
}

struct SystemResolver;

#[async_trait]
impl Resolve for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>> {
        let addresses = tokio::net::lookup_host((host, port)).await?;
        Ok(addresses.map(|address| address.ip()).collect())
    }
}

/// Fetches with curl; each hop is checked and the host is pinned to the checked address.
pub struct CurlCatalogFetcher {
    reach: Reach,
    resolver: Box<dyn Resolve>,
}

impl std::fmt::Debug for CurlCatalogFetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CurlCatalogFetcher")
            .field("reach", &self.reach)
            .finish_non_exhaustive()
    }
}

/// Host, port and every checked address of the host.
type Pin = (String, u16, Vec<IpAddr>);

struct Answer {
    status: u16,
    redirect: Option<String>,
    body: Vec<u8>,
}

impl CurlCatalogFetcher {
    #[must_use]
    pub fn new(reach: Reach) -> Self {
        Self {
            reach,
            resolver: Box::new(SystemResolver),
        }
    }

    /// The address curl connects to; a name must resolve to public addresses only.
    async fn pin(&self, url: &CatalogUrl) -> Result<Option<Pin>, FetchError> {
        let endpoint = url.endpoint().ok_or(FetchError::Refused)?;
        let host = match endpoint.host {
            EndpointHost::Address(_) => return Ok(None),
            EndpointHost::Name(name) => name,
        };
        let addresses = self
            .resolver
            .resolve(&host, endpoint.port)
            .await
            .map_err(|_| FetchError::Unreachable)?;
        if self.reach == Reach::Public && addresses.iter().any(|ip| !is_public_address(*ip)) {
            return Err(FetchError::Refused);
        }
        if addresses.is_empty() {
            return Err(FetchError::Unreachable);
        }
        Ok(Some((host, endpoint.port, addresses)))
    }

    /// The request as a curl config, so nothing about it appears on a command line.
    fn config(
        &self,
        url: &CatalogUrl,
        pin: Option<&Pin>,
        max_bytes: u64,
        limit: Duration,
    ) -> String {
        let quoted = |text: &str| text.replace('\\', "\\\\").replace('"', "\\\"");
        let protocols = match self.reach {
            Reach::Public => "=https",
            Reach::Anywhere => "=http,https",
        };
        let mut config = format!(
            "url = \"{}\"\nproto = \"{protocols}\"\nnoproxy = \"*\"\nmax-redirs = 0\nmax-time = {}\nconnect-timeout = {CONNECT_LIMIT_SECONDS}\nmax-filesize = {max_bytes}\nuser-agent = \"{USER_AGENT}\"\nwrite-out = \"\\n%{{http_code}}\\n%{{redirect_url}}\"\n",
            quoted(url.as_str()),
            limit.as_secs(),
        );
        if let Some((host, port, addresses)) = pin {
            let addresses: Vec<String> = addresses
                .iter()
                .map(|address| match address {
                    IpAddr::V4(address) => address.to_string(),
                    IpAddr::V6(address) => format!("[{address}]"),
                })
                .collect();
            let _ = writeln!(
                config,
                "resolve = \"{}:{port}:{}\"",
                quoted(host),
                addresses.join(",")
            );
        }
        config
    }

    async fn request(
        &self,
        url: &CatalogUrl,
        max_bytes: u64,
        limit: Duration,
    ) -> Result<Answer, FetchError> {
        let pin = self.pin(url).await?;
        let config = self.config(url, pin.as_ref(), max_bytes, limit);
        let mut command = tokio::process::Command::new("curl");
        // -q first: a curlrc of the person must not change the request
        command
            .args(["-q", "--silent", "--show-error", "--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|_| FetchError::Unreachable)?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(config.as_bytes())
                .await
                .map_err(|_| FetchError::Unreachable)?;
        }
        let mut output = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            let room = max_bytes.saturating_add(TRAILER_ROOM);
            stdout
                .take(room.saturating_add(1))
                .read_to_end(&mut output)
                .await
                .map_err(|_| FetchError::Unreachable)?;
            if output.len() as u64 > room {
                let _ = child.kill().await;
                return Err(FetchError::TooLarge);
            }
        }
        let exit = child.wait().await.map_err(|_| FetchError::Unreachable)?;
        match exit.code() {
            Some(0) => {}
            Some(28) => return Err(FetchError::Timeout),
            Some(63) => return Err(FetchError::TooLarge),
            Some(1 | 3) => return Err(FetchError::Refused),
            _ => return Err(FetchError::Unreachable),
        }
        let answer = split_answer(output).ok_or(FetchError::Unreachable)?;
        if answer.body.len() as u64 > max_bytes {
            return Err(FetchError::TooLarge);
        }
        Ok(answer)
    }

    async fn follow(&self, url: &CatalogUrl, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
        let mut current = url.clone();
        for _ in 0..=MAX_REDIRECTS {
            // each hop is checked as the first one was
            let target =
                CatalogUrl::parse(current.as_str(), self.reach).map_err(|_| FetchError::Refused)?;
            let answer = self
                .request(&target, max_bytes, limit_for(max_bytes))
                .await?;
            match answer.status {
                200..=299 => return Ok(answer.body),
                301 | 302 | 303 | 307 | 308 => {
                    let location = answer.redirect.ok_or(FetchError::Status(answer.status))?;
                    current = redirect_target(&location, self.reach)?;
                }
                status => return Err(FetchError::Status(status)),
            }
        }
        Err(FetchError::Refused)
    }
}

fn limit_for(max_bytes: u64) -> Duration {
    if max_bytes <= MAX_INDEX_BYTES {
        INDEX_LIMIT
    } else {
        PACKAGE_LIMIT
    }
}

/// Where a redirect leads, if that is a place a fetch may go to.
fn redirect_target(location: &str, reach: Reach) -> Result<CatalogUrl, FetchError> {
    CatalogUrl::parse(location, reach).map_err(|_| FetchError::Refused)
}

/// Body, status and redirect from the output: `body \n status \n redirect`.
fn split_answer(mut output: Vec<u8>) -> Option<Answer> {
    let last = output.iter().rposition(|byte| *byte == b'\n')?;
    let redirect = String::from_utf8_lossy(&output[last + 1..])
        .trim()
        .to_owned();
    output.truncate(last);
    let middle = output.iter().rposition(|byte| *byte == b'\n')?;
    let status: u16 = String::from_utf8_lossy(&output[middle + 1..])
        .trim()
        .parse()
        .ok()?;
    output.truncate(middle);
    Some(Answer {
        status,
        redirect: (!redirect.is_empty()).then_some(redirect),
        body: output,
    })
}

#[async_trait]
impl CatalogFetcher for CurlCatalogFetcher {
    async fn fetch(&self, url: &CatalogUrl, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
        tokio::time::timeout(limit_for(max_bytes), self.follow(url, max_bytes))
            .await
            .unwrap_or(Err(FetchError::Timeout))
    }
}

/// Fetches resources of themes: public https only, one request, redirects left to the caller.
pub struct CurlAssetFetcher {
    curl: CurlCatalogFetcher,
}

impl std::fmt::Debug for CurlAssetFetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CurlAssetFetcher").finish_non_exhaustive()
    }
}

impl CurlAssetFetcher {
    #[must_use]
    pub fn new() -> Self {
        Self {
            curl: CurlCatalogFetcher::new(Reach::Public),
        }
    }
}

impl Default for CurlAssetFetcher {
    fn default() -> Self {
        Self::new()
    }
}

fn asset_limit(max_bytes: u64) -> Duration {
    if max_bytes <= MAX_ASSET_BYTES {
        ASSET_LIMIT
    } else {
        ARCHIVE_LIMIT
    }
}

#[async_trait]
impl AssetFetcher for CurlAssetFetcher {
    async fn get(&self, url: &CatalogUrl, max_bytes: u64) -> Result<Hop, FetchError> {
        // the address is checked here again, whoever built it
        let url =
            CatalogUrl::parse(url.as_str(), self.curl.reach).map_err(|_| FetchError::Refused)?;
        let limit = asset_limit(max_bytes);
        let answer = tokio::time::timeout(limit, self.curl.request(&url, max_bytes, limit))
            .await
            .unwrap_or(Err(FetchError::Timeout))?;
        match answer.status {
            200..=299 => Ok(Hop::Body(answer.body)),
            301 | 302 | 303 | 307 | 308 => answer
                .redirect
                .map(Hop::Redirect)
                .ok_or(FetchError::Status(answer.status)),
            status => Err(FetchError::Status(status)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    /// Answers each request with the next response and reports the request texts.
    fn serve(responses: Vec<Vec<u8>>) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (sent, received) = mpsc::channel();
        thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                }
                let _ = sent.send(String::from_utf8_lossy(&request).into_owned());
                let _ = stream.write_all(&response);
            }
        });
        (base, received)
    }

    fn reply(status: &str, extra: &str, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn local(url: &str) -> CatalogUrl {
        CatalogUrl::parse(url, Reach::Anywhere).unwrap()
    }

    fn fetcher() -> CurlCatalogFetcher {
        CurlCatalogFetcher::new(Reach::Anywhere)
    }

    struct Fixed(std::io::Result<Vec<IpAddr>>);

    #[async_trait]
    impl Resolve for Fixed {
        async fn resolve(&self, _host: &str, _port: u16) -> std::io::Result<Vec<IpAddr>> {
            match &self.0 {
                Ok(addresses) => Ok(addresses.clone()),
                Err(error) => Err(std::io::Error::new(error.kind(), "lookup failed")),
            }
        }
    }

    fn resolving(reach: Reach, answer: &[&str]) -> CurlCatalogFetcher {
        let addresses = answer.iter().map(|text| text.parse().unwrap()).collect();
        CurlCatalogFetcher {
            reach,
            resolver: Box::new(Fixed(Ok(addresses))),
        }
    }

    #[tokio::test]
    async fn downloads_a_body_byte_for_byte_and_sends_no_credentials() {
        let body: Vec<u8> = vec![0, 13, 10, 255, b'\n', 7, b'\r', b'\n'];
        let (base, request) = serve(vec![reply("200 OK", "", &body)]);
        let bytes = fetcher()
            .fetch(&local(&format!("{base}/i.json")), 1024)
            .await
            .unwrap();
        assert_eq!(bytes, body);
        let request = request.recv().unwrap().to_lowercase();
        assert!(request.starts_with("get /i.json "));
        assert!(request.contains("user-agent: nullradix-catalog"));
        for header in ["authorization", "cookie", "proxy-authorization"] {
            assert!(!request.contains(header), "{header}");
        }
    }

    #[tokio::test]
    async fn a_body_over_the_limit_is_too_large_known_or_not() {
        let (base, _) = serve(vec![reply("200 OK", "", &[b'a'; 200])]);
        let error = fetcher()
            .fetch(&local(&format!("{base}/big")), 100)
            .await
            .unwrap_err();
        assert_eq!(error, FetchError::TooLarge);

        // no length: the body ends with the connection
        let chunked = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        let mut unknown = chunked;
        unknown.extend_from_slice(&[b'a'; 200]);
        let (base, _) = serve(vec![unknown]);
        let error = fetcher()
            .fetch(&local(&format!("{base}/big")), 100)
            .await
            .unwrap_err();
        assert_eq!(error, FetchError::TooLarge);

        let (base, _) = serve(vec![reply("200 OK", "", &[b'a'; 100])]);
        let bytes = fetcher()
            .fetch(&local(&format!("{base}/exact")), 100)
            .await
            .unwrap();
        assert_eq!(bytes.len(), 100);
    }

    #[tokio::test]
    async fn error_statuses_are_reported() {
        let (base, _) = serve(vec![reply("404 Not Found", "", b"gone")]);
        let error = fetcher()
            .fetch(&local(&format!("{base}/x")), 100)
            .await
            .unwrap_err();
        assert_eq!(error, FetchError::Status(404));
    }

    #[tokio::test]
    async fn an_unreachable_server_is_unreachable() {
        let address = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let error = fetcher()
            .fetch(&local(&format!("http://{address}/x")), 100)
            .await
            .unwrap_err();
        assert_eq!(error, FetchError::Unreachable);
    }

    #[tokio::test]
    async fn redirects_are_followed_a_few_times() {
        let (base, requests) = serve(vec![
            reply("302 Found", "Location: /second\r\n", b""),
            reply("307 Temporary Redirect", "Location: /third\r\n", b""),
            reply("200 OK", "", b"done"),
        ]);
        let bytes = fetcher()
            .fetch(&local(&format!("{base}/first")), 100)
            .await
            .unwrap();
        assert_eq!(bytes, b"done");
        let paths: Vec<String> = requests
            .try_iter()
            .map(|text| text.split_whitespace().nth(1).unwrap().to_owned())
            .collect();
        assert_eq!(paths, ["/first", "/second", "/third"]);
    }

    #[tokio::test]
    async fn too_many_redirects_and_redirects_to_other_schemes_are_refused() {
        let hop = |location: &str| reply("302 Found", &format!("Location: {location}\r\n"), b"");
        let (base, _) = serve(vec![hop("/a"), hop("/b"), hop("/c"), hop("/d"), hop("/e")]);
        let error = fetcher()
            .fetch(&local(&format!("{base}/start")), 100)
            .await
            .unwrap_err();
        assert_eq!(error, FetchError::Refused);

        for location in [
            "file:///etc/passwd",
            "ftp://127.0.0.1/x",
            "http://user:pass@127.0.0.1/x",
            "gopher://127.0.0.1/x",
        ] {
            let (base, requests) = serve(vec![hop(location)]);
            let error = fetcher()
                .fetch(&local(&format!("{base}/start")), 100)
                .await
                .unwrap_err();
            assert_eq!(error, FetchError::Refused, "{location}");
            assert_eq!(requests.try_iter().count(), 1, "{location}");
        }
    }

    #[test]
    fn a_public_fetch_follows_redirects_only_to_public_https() {
        for target in [
            "http://example.com/x",
            "https://127.0.0.1/x",
            "https://localhost/x",
            "https://10.0.0.1/x",
            "https://169.254.169.254/latest",
            "https://example.com:8443/x",
            "https://u:p@example.com/x",
            "file:///etc/passwd",
            "/relative",
        ] {
            assert_eq!(
                redirect_target(target, Reach::Public).unwrap_err(),
                FetchError::Refused,
                "{target}"
            );
        }
        assert!(redirect_target("https://cdn.example.com/x.zip", Reach::Public).is_ok());
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_a_private_address_is_never_requested() {
        let url = CatalogUrl::parse("https://catalog.example.com/i.json", Reach::Public).unwrap();
        for answer in [
            vec!["10.0.0.1"],
            vec!["127.0.0.1"],
            vec!["169.254.169.254"],
            vec!["8.8.8.8", "192.168.1.1"],
            vec!["::1"],
            vec!["fd00::1"],
        ] {
            let error = resolving(Reach::Public, &answer)
                .fetch(&url, 100)
                .await
                .unwrap_err();
            assert_eq!(error, FetchError::Refused, "{answer:?}");
        }
    }

    #[tokio::test]
    async fn a_failed_or_empty_lookup_is_unreachable() {
        let url = CatalogUrl::parse("https://catalog.example.com/i.json", Reach::Public).unwrap();
        let empty = resolving(Reach::Public, &[]);
        assert_eq!(
            empty.fetch(&url, 100).await.unwrap_err(),
            FetchError::Unreachable
        );
        let failing = CurlCatalogFetcher {
            reach: Reach::Public,
            resolver: Box::new(Fixed(Err(std::io::ErrorKind::NotFound.into()))),
        };
        assert_eq!(
            failing.fetch(&url, 100).await.unwrap_err(),
            FetchError::Unreachable
        );
    }

    #[tokio::test]
    async fn curl_connects_to_the_address_that_was_checked() {
        let (base, _) = serve(vec![reply("200 OK", "", b"pinned")]);
        let port = base.rsplit(':').next().unwrap();
        // the name exists nowhere else: only the pinned address can answer
        let url = local(&format!("http://catalog.invalid:{port}/i.json"));
        let bytes = resolving(Reach::Anywhere, &["127.0.0.1"])
            .fetch(&url, 100)
            .await
            .unwrap();
        assert_eq!(bytes, b"pinned");
    }

    #[test]
    fn the_config_quotes_the_address_and_pins_the_host() {
        let url = local("http://catalog.test:81/a\"b");
        let pin = (
            "catalog.test".to_owned(),
            81,
            vec!["::1".parse().unwrap(), "10.1.2.3".parse().unwrap()],
        );
        let config = fetcher().config(&url, Some(&pin), 5, limit_for(5));
        assert!(config.contains("a%22b") || config.contains("a\\\"b"));
        assert!(config.contains("resolve = \"catalog.test:81:[::1],10.1.2.3\""));
        let odd = ("a\"b\\c".to_owned(), 81, vec!["10.1.2.3".parse().unwrap()]);
        let escaped = fetcher().config(&url, Some(&odd), 5, limit_for(5));
        assert!(escaped.contains("resolve = \"a\\\"b\\\\c:81:10.1.2.3\""));
        assert!(config.contains("noproxy = \"*\""));
        assert!(config.contains("max-redirs = 0"));
        assert!(config.contains("max-filesize = 5"));
        let strict = CurlCatalogFetcher::new(Reach::Public);
        assert!(
            strict
                .config(&url, None, 5, limit_for(5))
                .contains("proto = \"=https\"")
        );
    }

    #[test]
    fn an_index_gets_less_time_than_a_package() {
        assert_eq!(limit_for(MAX_INDEX_BYTES), INDEX_LIMIT);
        assert_eq!(limit_for(MAX_INDEX_BYTES + 1), PACKAGE_LIMIT);
    }

    fn assets(curl: CurlCatalogFetcher) -> CurlAssetFetcher {
        CurlAssetFetcher { curl }
    }

    #[tokio::test]
    async fn an_asset_redirect_is_handed_back_and_not_followed() {
        let (base, requests) = serve(vec![
            reply("302 Found", "Location: /next/b.png\r\n", b""),
            reply("200 OK", "", b"never asked"),
        ]);
        let hop = assets(fetcher())
            .get(&local(&format!("{base}/a.png")), 100)
            .await
            .unwrap();
        assert_eq!(hop, Hop::Redirect(format!("{base}/next/b.png")));
        assert_eq!(requests.try_iter().count(), 1);

        let (base, _) = serve(vec![reply("302 Found", "", b"")]);
        let error = assets(fetcher())
            .get(&local(&format!("{base}/a.png")), 100)
            .await
            .unwrap_err();
        assert_eq!(error, FetchError::Status(302));
    }

    #[tokio::test]
    async fn an_asset_body_is_returned_without_credentials_and_statuses_are_reported() {
        let (base, request) = serve(vec![
            reply("200 OK", "", b"PNG"),
            reply("404 Not Found", "", b"gone"),
            reply("500 Oops", "", b""),
        ]);
        let fetcher = assets(fetcher());
        let hop = fetcher
            .get(&local(&format!("{base}/a.png")), 100)
            .await
            .unwrap();
        assert_eq!(hop, Hop::Body(b"PNG".to_vec()));
        let request = request.recv().unwrap().to_lowercase();
        for header in [
            "authorization",
            "cookie",
            "proxy-authorization",
            "referer",
            "origin",
        ] {
            assert!(!request.contains(header), "{header}");
        }
        for status in [404, 500] {
            let error = fetcher
                .get(&local(&format!("{base}/b.png")), 100)
                .await
                .unwrap_err();
            assert_eq!(error, FetchError::Status(status));
        }
    }

    #[tokio::test]
    async fn an_asset_just_above_the_limit_is_too_large_known_or_not() {
        let (base, _) = serve(vec![
            reply("200 OK", "", &[b'a'; 100]),
            reply("200 OK", "", &[b'a'; 101]),
        ]);
        let fetcher = assets(fetcher());
        let at = fetcher
            .get(&local(&format!("{base}/a")), 100)
            .await
            .unwrap();
        assert_eq!(at, Hop::Body(vec![b'a'; 100]));
        let above = fetcher.get(&local(&format!("{base}/b")), 100).await;
        assert_eq!(above.unwrap_err(), FetchError::TooLarge);

        let mut unknown = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        unknown.extend_from_slice(&[b'a'; 101]);
        let (base, _) = serve(vec![unknown]);
        let above = assets(self::fetcher())
            .get(&local(&format!("{base}/c")), 100)
            .await;
        assert_eq!(above.unwrap_err(), FetchError::TooLarge);
    }

    #[tokio::test]
    async fn an_asset_is_fetched_from_the_address_that_was_checked() {
        let (base, _) = serve(vec![reply("200 OK", "", b"pinned")]);
        let port = base.rsplit(':').next().unwrap();
        let url = local(&format!("http://assets.invalid:{port}/a.png"));
        let hop = assets(resolving(Reach::Anywhere, &["127.0.0.1"]))
            .get(&url, 100)
            .await
            .unwrap();
        assert_eq!(hop, Hop::Body(b"pinned".to_vec()));
    }

    #[tokio::test]
    async fn an_asset_host_that_resolves_to_a_private_address_is_refused_before_connecting() {
        let url = CatalogUrl::parse("https://assets.example.com/a.png", Reach::Public).unwrap();
        for answer in [
            vec!["10.0.0.1"],
            vec!["127.0.0.1"],
            vec!["169.254.169.254"],
            vec!["192.168.1.1"],
            vec!["100.64.0.1"],
            vec!["8.8.8.8", "192.168.1.1"],
            vec!["::1"],
            vec!["fd00::1"],
            vec!["fe80::1"],
            vec!["::ffff:10.0.0.1"],
        ] {
            let error = assets(resolving(Reach::Public, &answer))
                .get(&url, 100)
                .await
                .unwrap_err();
            assert_eq!(error, FetchError::Refused, "{answer:?}");
        }
        let empty = assets(resolving(Reach::Public, &[]));
        assert_eq!(
            empty.get(&url, 100).await.unwrap_err(),
            FetchError::Unreachable
        );
    }

    #[tokio::test]
    async fn an_address_outside_the_public_reach_is_never_connected_to() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let fetcher = assets(CurlCatalogFetcher::new(Reach::Public));
        for raw in [
            format!("http://127.0.0.1:{port}/a.png"),
            format!("https://127.0.0.1:{port}/a.png"),
            String::from("https://localhost/a.png"),
            String::from("https://[::1]/a.png"),
            String::from("https://169.254.169.254/latest"),
        ] {
            let url = local(&raw);
            let error = fetcher.get(&url, 100).await.unwrap_err();
            assert_eq!(error, FetchError::Refused, "{raw}");
        }
        let accepted = listener.accept();
        assert_eq!(accepted.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn an_archive_gets_more_time_than_a_resource() {
        assert_eq!(asset_limit(MAX_ASSET_BYTES), ASSET_LIMIT);
        assert_eq!(asset_limit(MAX_ASSET_BYTES + 1), ARCHIVE_LIMIT);
    }

    #[test]
    fn splits_body_status_and_redirect() {
        let answer = split_answer(b"a\nb\n302\nhttps://x/y".to_vec()).unwrap();
        assert_eq!(
            (answer.body, answer.status, answer.redirect.as_deref()),
            (b"a\nb".to_vec(), 302, Some("https://x/y"))
        );
        let plain = split_answer(b"body\n200\n".to_vec()).unwrap();
        assert_eq!(
            (plain.body, plain.status, plain.redirect),
            (b"body".to_vec(), 200, None)
        );
        assert!(split_answer(b"garbage".to_vec()).is_none());
        assert!(split_answer(b"x\nabc\n".to_vec()).is_none());
    }
}
