use std::net::SocketAddr;
use std::time::Duration;

use nullradix_api::{
    API_PREFIX, API_VERSION, CreateProject, Handoff, Health, Problem, ProjectDto, ProjectList,
    RenameProject, RunDto, RunList, RunOutput, StartRun,
};
use nullradix_app::auth::Secret;
use serde::de::DeserializeOwned;
use ureq::http::Response;
use ureq::{Agent, Body};
use uuid::Uuid;

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
    #[error("no local token, start the daemon once to create it")]
    MissingToken,
    #[error("the daemon refused the request: {detail} ({code})")]
    Refused {
        status: u16,
        code: String,
        detail: String,
    },
    #[error("unexpected response from the daemon: {0}")]
    Protocol(String),
}

/// Blocking HTTP client for short-lived callers such as the CLI and hooks.
#[derive(Debug, Clone)]
pub struct Client {
    agent: Agent,
    address: SocketAddr,
    token: Option<Secret>,
}

impl Client {
    #[must_use]
    pub fn new(address: SocketAddr, connect_timeout: Duration, timeout: Duration) -> Self {
        let agent = Agent::config_builder()
            .timeout_connect(Some(connect_timeout))
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .proxy(None)
            .build()
            .into();
        Self {
            agent,
            address,
            token: None,
        }
    }

    #[must_use]
    pub fn with_token(mut self, token: Secret) -> Self {
        self.token = Some(token);
        self
    }

    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn health(&self) -> Result<Health, ClientError> {
        let mut response = self
            .agent
            .get(&self.url("/health"))
            .call()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
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

    /// One-time code that opens a browser session.
    pub fn create_handoff(&self) -> Result<Handoff, ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/auth/handoffs"))
            .header("Authorization", &self.authorization()?)
            .send_empty()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    pub fn list_projects(&self) -> Result<ProjectList, ClientError> {
        self.get_json("/projects")
    }

    pub fn create_project(&self, input: &CreateProject) -> Result<ProjectDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/projects"))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    pub fn rename_project(
        &self,
        id: Uuid,
        input: &RenameProject,
    ) -> Result<ProjectDto, ClientError> {
        let mut response = self
            .agent
            .patch(&self.url(&format!("/projects/{id}")))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    pub fn delete_project(&self, id: Uuid, version: i64) -> Result<(), ClientError> {
        let mut response = self
            .agent
            .delete(&self.url(&format!("/projects/{id}?version={version}")))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        expect_empty(&mut response, 204)
    }

    pub fn list_runs(&self, limit: u32) -> Result<RunList, ClientError> {
        self.get_json(&format!("/runs?limit={limit}"))
    }

    pub fn start_run(&self, input: &StartRun) -> Result<RunDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url("/runs"))
            .header("Authorization", &self.authorization()?)
            .send_json(input)
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 201)
    }

    pub fn get_run(&self, id: Uuid) -> Result<RunDto, ClientError> {
        self.get_json(&format!("/runs/{id}"))
    }

    pub fn run_output(&self, id: Uuid, after: i64) -> Result<RunOutput, ClientError> {
        self.get_json(&format!("/runs/{id}/output?after={after}"))
    }

    pub fn cancel_run(&self, id: Uuid) -> Result<RunDto, ClientError> {
        let mut response = self
            .agent
            .post(&self.url(&format!("/runs/{id}/cancel")))
            .header("Authorization", &self.authorization()?)
            .send_empty()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 202)
    }

    fn get_json<T: DeserializeOwned>(&self, route: &str) -> Result<T, ClientError> {
        let mut response = self
            .agent
            .get(&self.url(route))
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|source| self.unreachable(source))?;
        decode(&mut response, 200)
    }

    fn authorization(&self) -> Result<String, ClientError> {
        let token = self.token.as_ref().ok_or(ClientError::MissingToken)?;
        Ok(format!("Bearer {}", token.expose()))
    }

    fn url(&self, route: &str) -> String {
        format!("http://{}{API_PREFIX}{route}", self.address)
    }

    fn unreachable(&self, source: ureq::Error) -> ClientError {
        ClientError::Unreachable {
            address: self.address,
            source: Box::new(source),
        }
    }
}

fn expect_empty(response: &mut Response<Body>, expected: u16) -> Result<(), ClientError> {
    if response.status().as_u16() == expected {
        return Ok(());
    }
    decode::<serde_json::Value>(response, expected).map(|_| ())
}

fn decode<T: DeserializeOwned>(
    response: &mut Response<Body>,
    expected: u16,
) -> Result<T, ClientError> {
    let status = response.status().as_u16();
    if status == expected {
        return response
            .body_mut()
            .read_json()
            .map_err(|error| ClientError::Protocol(error.to_string()));
    }
    match response.body_mut().read_json::<Problem>() {
        Ok(problem) => Err(ClientError::Refused {
            status,
            code: problem.code,
            detail: problem.detail,
        }),
        Err(_) => Err(ClientError::Protocol(format!("unexpected status {status}"))),
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::Arc;

    use nullradix_api::{AllowedHosts, ApiState};
    use nullradix_app::auth::AuthService;
    use nullradix_app::projects::ProjectService;
    use nullradix_app::runs::RunService;
    use nullradix_app::{ChangeSignal, SystemClock, UuidV7Generator};
    use nullradix_runner::{LocalRunner, SupervisorCommand};
    use nullradix_store::Store;

    use super::*;

    const FAST: Duration = Duration::from_millis(500);

    struct TestDaemon {
        address: SocketAddr,
        token: Secret,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
    }

    impl Drop for TestDaemon {
        fn drop(&mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
        }
    }

    fn spawn_daemon() -> TestDaemon {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let (ready, token) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let store = Arc::new(Store::in_memory().await.unwrap());
                let auth = Arc::new(AuthService::new(
                    store.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                ));
                ready.send(auth.local_token(None).await.unwrap()).unwrap();
                let signal = ChangeSignal::new();
                let projects = Arc::new(ProjectService::new(
                    store.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                    signal.clone(),
                ));
                let runs = RunService::new(
                    store.clone(),
                    Arc::new(LocalRunner::new(
                        std::env::temp_dir().join("nullradix-client-tests"),
                        SupervisorCommand {
                            program: std::path::PathBuf::from("nullradix-missing-supervisor"),
                            args: Vec::new(),
                        },
                    )),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                    signal.clone(),
                    FAST,
                );
                let state = ApiState {
                    auth,
                    projects,
                    runs,
                    changes: store,
                    signal,
                    shutdown: tokio_util::sync::CancellationToken::new(),
                    allowed_hosts: AllowedHosts::loopback_and([]),
                    secure_cookies: false,
                    web_dir: None,
                };
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let shutdown = async {
                    let _ = stopped.await;
                };
                nullradix_api::serve(listener, nullradix_api::router(state), shutdown)
                    .await
                    .unwrap();
            });
        });
        TestDaemon {
            address,
            token: token.recv().unwrap(),
            stop: Some(stop),
        }
    }

    #[test]
    fn handshakes_with_a_running_daemon() {
        let daemon = spawn_daemon();
        let health = Client::new(daemon.address, FAST, FAST).handshake().unwrap();
        assert_eq!(health.api_version, API_VERSION);
        assert_eq!(health.status, "ok");
    }

    #[test]
    fn creates_handoffs_with_the_local_token_only() {
        let daemon = spawn_daemon();
        let client = Client::new(daemon.address, FAST, FAST);
        assert!(matches!(
            client.create_handoff(),
            Err(ClientError::MissingToken)
        ));

        let handoff = client
            .clone()
            .with_token(daemon.token.clone())
            .create_handoff()
            .unwrap();
        assert!(handoff.path.ends_with(&handoff.code));

        let stranger = Secret::parse(&"c".repeat(64)).unwrap();
        let refused = client.with_token(stranger).create_handoff().unwrap_err();
        assert!(matches!(refused, ClientError::Refused { status: 401, .. }));
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
