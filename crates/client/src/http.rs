use std::net::SocketAddr;
use std::time::Duration;

use nullradix_api::{API_VERSION, Health};
use ureq::Agent;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("the daemon is not reachable at {address}")]
    Unreachable {
        address: SocketAddr,
        #[source]
        source: Box<ureq::Error>,
    },
    #[error("the daemon speaks API version {found}, this client needs {expected}")]
    VersionMismatch { found: u32, expected: u32 },
    #[error("unexpected response from the daemon: {0}")]
    Protocol(String),
}

/// Blocking HTTP client for short-lived callers such as the CLI and hooks.
#[derive(Debug, Clone)]
pub struct Client {
    agent: Agent,
    address: SocketAddr,
}

impl Client {
    #[must_use]
    pub fn new(address: SocketAddr, connect_timeout: Duration, timeout: Duration) -> Self {
        let agent = Agent::config_builder()
            .timeout_connect(Some(connect_timeout))
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .into();
        Self { agent, address }
    }

    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn health(&self) -> Result<Health, ClientError> {
        let url = format!("http://{}/health", self.address);
        let mut response =
            self.agent
                .get(&url)
                .call()
                .map_err(|source| ClientError::Unreachable {
                    address: self.address,
                    source: Box::new(source),
                })?;
        if response.status() != 200 {
            return Err(ClientError::Protocol(format!(
                "health check answered {}",
                response.status()
            )));
        }
        response
            .body_mut()
            .read_json()
            .map_err(|error| ClientError::Protocol(error.to_string()))
    }

    /// Health check that also refuses a daemon with another API major version.
    pub fn handshake(&self) -> Result<Health, ClientError> {
        let health = self.health()?;
        if health.api_version != API_VERSION {
            return Err(ClientError::VersionMismatch {
                found: health.api_version,
                expected: API_VERSION,
            });
        }
        Ok(health)
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    const FAST: Duration = Duration::from_millis(500);

    fn spawn_daemon() -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let shutdown = async {
                    let _ = stopped.await;
                };
                nullradix_api::serve(listener, nullradix_api::router(), shutdown)
                    .await
                    .unwrap();
            });
        });
        (address, stop)
    }

    #[test]
    fn handshakes_with_a_running_daemon() {
        let (address, stop) = spawn_daemon();
        let health = Client::new(address, FAST, FAST).handshake().unwrap();
        assert_eq!(health.api_version, API_VERSION);
        assert_eq!(health.status, "ok");
        stop.send(()).unwrap();
    }

    #[test]
    fn reports_an_unreachable_daemon() {
        let address = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let error = Client::new(address, FAST, FAST).health().unwrap_err();
        assert!(matches!(error, ClientError::Unreachable { .. }));
    }
}
