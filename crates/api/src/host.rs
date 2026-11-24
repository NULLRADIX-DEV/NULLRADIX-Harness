use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::uri::Authority;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;

use crate::problem;

/// Host names the API answers to; other names are refused to stop DNS rebinding.
#[derive(Debug, Clone)]
pub struct AllowedHosts(Arc<[String]>);

impl AllowedHosts {
    /// Loopback names plus `extra`, compared without port and case.
    #[must_use]
    pub fn loopback_and(extra: impl IntoIterator<Item = String>) -> Self {
        let mut hosts: Vec<String> = ["localhost", "127.0.0.1", "[::1]"]
            .into_iter()
            .map(String::from)
            .chain(extra.into_iter().map(|host| host.to_ascii_lowercase()))
            .collect();
        hosts.sort_unstable();
        hosts.dedup();
        Self(hosts.into())
    }

    pub(crate) fn allows(&self, authority: &str) -> bool {
        let host = host_name(authority).to_ascii_lowercase();
        self.0.contains(&host)
    }
}

fn host_name(authority: &str) -> &str {
    if authority.starts_with('[') {
        return authority
            .find(']')
            .map_or(authority, |end| &authority[..=end]);
    }
    authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host)
}

pub(crate) async fn check_host(
    State(hosts): State<AllowedHosts>,
    request: Request,
    next: Next,
) -> Response {
    let authority = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| request.uri().authority().map(Authority::as_str));
    match authority {
        Some(authority) if hosts.allows(authority) => next.run(request).await,
        _ => problem::response(
            StatusCode::FORBIDDEN,
            "host-not-allowed",
            "requests must use an allowed host name",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ports_and_keeps_ipv6_brackets() {
        assert_eq!(host_name("localhost:4711"), "localhost");
        assert_eq!(host_name("localhost"), "localhost");
        assert_eq!(host_name("[::1]:4711"), "[::1]");
        assert_eq!(host_name("[::1]"), "[::1]");
    }

    #[test]
    fn allows_loopback_and_configured_hosts_only() {
        let hosts = AllowedHosts::loopback_and([String::from("Workbench.Example.com")]);
        assert!(hosts.allows("LOCALHOST:4711"));
        assert!(hosts.allows("127.0.0.1:4711"));
        assert!(hosts.allows("[::1]:4711"));
        assert!(hosts.allows("workbench.example.com"));
        assert!(!hosts.allows("attacker.example.com:4711"));
        assert!(!hosts.allows("127.0.0.1.attacker.example.com"));
    }
}
